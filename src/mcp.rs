//! `decisions mcp`: an MCP server over stdio (JSON-RPC 2.0, one message per line). It files
//! questions in the store and waits for answers in bounded slices, so a client's tool timeout
//! never loses a question: what is not answered in time comes back as `pending`, and
//! `wait_for_answer` picks it up later from any server process.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::{Config, Mode};
use crate::store::{New, Outcome, Store, is_key};

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const PING_EVERY: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(200);

const ALWAYS: &str = "When you need a decision, approval or clarification from the user, call ask_user instead of asking in the chat. Keep working on anything that does not depend on the answer; when you need it, call wait_for_answer.";
const ON_REQUEST: &str = "If the user asks you to send your questions to the decisions app, use ask_user for them from then on instead of asking in the chat.";

struct Server<W: Write> {
    store: Store,
    mode: Mode,
    wait: u64,
    session: Option<String>,
    cwd_name: String,
    ping_every: Duration,
    out: Mutex<W>,
    client: Mutex<String>,
    cancelled: Mutex<HashSet<String>>,
    reported: Mutex<HashMap<String, String>>,
    closed: AtomicBool,
}

fn tools() -> Value {
    json!([
        {
            "name": "ask_user",
            "description": "Ask the user a question and wait for the answer in their decisions app, a queue of questions from all their agent sessions. Use it instead of asking in the chat for any decision, approval or clarification. Give 2-8 short options; the user can always answer in their own words, so do not add an \"Other\" option. Put what the user needs to decide in `context` (markdown). Returns the answer, or `pending` with an id when the user has not answered yet: keep working on what does not depend on it and call wait_for_answer when you need it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "One short question."},
                    "options": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 8, "description": "2-8 short answers to pick from."},
                    "recommended": {"type": "integer", "minimum": 1, "description": "1-based number of the option you recommend."},
                    "context": {"type": "string", "description": "Markdown: what the user needs to know to decide."},
                    "link": {"type": "string", "description": "A URL the user may open (e.g. the merge request)."},
                    "ticket": {"type": "string", "description": "Ticket key, e.g. ABC-123."},
                    "blocks": {"type": "string", "description": "What waits on the answer, in a word or two (e.g. merge, reply)."}
                },
                "required": ["question", "options"]
            }
        },
        {
            "name": "wait_for_answer",
            "description": "Wait for the answer to a question asked with ask_user. Returns the answer, the user's question back, `withdrawn`, or `pending` again when the user has still not answered.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}, "timeout_seconds": {"type": "integer", "minimum": 1}}, "required": ["id"]}
        },
        {
            "name": "reply",
            "description": "Answer a question the user asked back about one of your questions (the wait returned 'the user asked back'). Then call wait_for_answer again.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}, "text": {"type": "string"}}, "required": ["id", "text"]}
        },
        {
            "name": "withdraw",
            "description": "Withdraw a question you no longer need answered; it leaves the user's queue.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}, "reason": {"type": "string"}}, "required": ["id"]}
        }
    ])
}

fn text_result(text: String, structured: Value) -> Value {
    json!({"content": [{"type": "text", "text": text}], "structuredContent": structured, "isError": false})
}

fn outcome_result(id: &str, o: &Outcome) -> Value {
    let structured = match o {
        Outcome::Pending => json!({"status": "pending", "id": id}),
        Outcome::Answered { n, text, note } => {
            json!({"status": "answered", "id": id, "option": n, "text": text, "note": note})
        }
        Outcome::Told(text) => json!({"status": "answered_own_words", "id": id, "text": text}),
        Outcome::AskedBack(q) => json!({"status": "asked_back", "id": id, "question": q}),
        Outcome::Withdrawn(reason) => json!({"status": "withdrawn", "id": id, "reason": reason}),
    };
    text_result(o.describe(id), structured)
}

fn str_arg(args: &Value, name: &str) -> Result<String, String> {
    args[name]
        .as_str()
        .map(String::from)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("`{name}` is required"))
}

impl<W: Write> Server<W> {
    fn send(&self, msg: Value) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{msg}");
            let _ = out.flush();
        }
    }

    fn source(&self) -> String {
        let client = self.client.lock().map(|c| c.clone()).unwrap_or_default();
        let mut s = format!(
            "{} · {}",
            if client.is_empty() { "agent" } else { &client },
            self.cwd_name
        );
        if let Some(label) = &self.session {
            s.push_str(&format!(" · {label}"));
        }
        s
    }

    fn request(&self, id: Value, method: &str, params: &Value) {
        let result = match method {
            "initialize" => {
                if let (Some(name), Ok(mut c)) =
                    (params["clientInfo"]["name"].as_str(), self.client.lock())
                {
                    *c = name.to_string();
                }
                let asked = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
                let version = if SUPPORTED.contains(&asked) {
                    asked
                } else {
                    PROTOCOL
                };
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "decisions", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": match self.mode { Mode::Always => ALWAYS, Mode::OnRequest => ON_REQUEST },
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools()}),
            _ => {
                return self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}}));
            }
        };
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn call(&self, id: Value, params: &Value) {
        let key = id.to_string();
        let token = params["_meta"]["progressToken"].clone();
        let args = &params["arguments"];
        let result = match params["name"].as_str().unwrap_or("") {
            "ask_user" => self
                .ask_user(args)
                .and_then(|qid| self.wait_for(&qid, None, &token, &key)),
            "wait_for_answer" => str_arg(args, "id").and_then(|qid| {
                self.wait_for(&qid, args["timeout_seconds"].as_u64(), &token, &key)
            }),
            "reply" => str_arg(args, "id").and_then(|qid| {
                let text = str_arg(args, "text")?;
                self.store.reply(&qid, &text).map_err(|e| e.0)?;
                Ok(text_result(
                    format!("replied on {qid}; call wait_for_answer(\"{qid}\") for the answer"),
                    json!({"status": "ok", "id": qid}),
                ))
            }),
            "withdraw" => str_arg(args, "id").and_then(|qid| {
                self.store
                    .withdraw(&qid, args["reason"].as_str().unwrap_or(""))
                    .map_err(|e| e.0)?;
                Ok(text_result(
                    format!("withdrew {qid}"),
                    json!({"status": "ok", "id": qid}),
                ))
            }),
            other => Err(format!("unknown tool {other}")),
        };
        if self
            .cancelled
            .lock()
            .map(|mut c| c.remove(&key))
            .unwrap_or(false)
        {
            return;
        }
        let result = result
            .unwrap_or_else(|e| json!({"content": [{"type": "text", "text": e}], "isError": true}));
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn ask_user(&self, a: &Value) -> Result<String, String> {
        let question = str_arg(a, "question")?;
        let options: Vec<String> = a["options"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|o| o.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !(2..=8).contains(&options.len()) {
            return Err(
                "give 2 to 8 options; the user can always answer in their own words".into(),
            );
        }
        let ticket = a["ticket"].as_str().unwrap_or("").trim().to_string();
        let (key, mut context) = if ticket.is_empty() || is_key(&ticket.to_uppercase()) {
            (ticket, String::new())
        } else {
            (String::new(), format!("Ticket: {ticket}\n\n"))
        };
        context.push_str(a["context"].as_str().unwrap_or(""));
        if let Some(id) = self.store.find_open(&key, &question) {
            return Ok(id);
        }
        self.store
            .add(New {
                key,
                mr: a["link"].as_str().unwrap_or("").into(),
                blocks: a["blocks"].as_str().unwrap_or("").into(),
                source: self.source(),
                question,
                options,
                recommend: a["recommended"].as_u64().map(|n| n as usize),
                context,
            })
            .map_err(|e| e.0)
    }

    fn wait_for(
        &self,
        qid: &str,
        timeout: Option<u64>,
        token: &Value,
        key: &str,
    ) -> Result<Value, String> {
        let budget = match (self.wait, timeout) {
            (0, t) => t,
            (w, Some(t)) => Some(t.min(w)),
            (w, None) => Some(w),
        };
        let deadline = budget.map(|s| Instant::now() + Duration::from_secs(s));
        let mut next_ping = Instant::now() + self.ping_every;
        let mut progress = 0u64;
        loop {
            let outcome = self.store.outcome(qid).map_err(|e| e.0)?;
            let fresh = match &outcome {
                Outcome::Pending => false,
                Outcome::AskedBack(q) => self
                    .reported
                    .lock()
                    .map(|mut r| r.insert(qid.to_string(), q.clone()).as_ref() != Some(q))
                    .unwrap_or(true),
                _ => true,
            };
            if fresh {
                return Ok(outcome_result(qid, &outcome));
            }
            let cancelled = self
                .cancelled
                .lock()
                .map(|c| c.contains(key))
                .unwrap_or(false);
            if deadline.is_some_and(|d| Instant::now() >= d)
                || cancelled
                || self.closed.load(Ordering::Relaxed)
            {
                return Ok(outcome_result(qid, &Outcome::Pending));
            }
            if !token.is_null() && Instant::now() >= next_ping {
                progress += 1;
                self.send(json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": token, "progress": progress, "message": format!("waiting for the user to answer {qid}")}}));
                next_ping += self.ping_every;
            }
            thread::sleep(POLL);
        }
    }
}

pub fn serve<R: BufRead, W: Write + Send + 'static>(
    input: R,
    output: W,
    cfg: &Config,
) -> std::io::Result<()> {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    serve_with(input, output, cfg, PING_EVERY, cwd)
}

pub fn serve_with<R: BufRead, W: Write + Send + 'static>(
    input: R,
    output: W,
    cfg: &Config,
    ping_every: Duration,
    cwd_name: String,
) -> std::io::Result<()> {
    let server = Arc::new(Server {
        store: cfg.store(),
        mode: cfg.mode,
        wait: cfg.wait,
        session: cfg.session.clone(),
        cwd_name,
        ping_every,
        out: Mutex::new(output),
        client: Mutex::new(String::new()),
        cancelled: Mutex::new(HashSet::new()),
        reported: Mutex::new(HashMap::new()),
        closed: AtomicBool::new(false),
    });
    let mut workers = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            server.send(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}));
            continue;
        };
        let method = msg["method"].as_str().unwrap_or("").to_string();
        match (method.as_str(), msg.get("id").cloned()) {
            ("tools/call", Some(id)) => {
                let s = server.clone();
                workers.push(thread::spawn(move || s.call(id, &msg["params"])));
            }
            ("notifications/cancelled", _) => {
                if let Ok(mut c) = server.cancelled.lock() {
                    c.insert(msg["params"]["requestId"].to_string());
                }
            }
            (_, Some(id)) => server.request(id, &method, &msg["params"]),
            _ => {}
        }
    }
    server.closed.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Mode};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering as O};
    use std::thread::JoinHandle;

    fn cfg(wait: u64, mode: Mode) -> Config {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "decisions-mcp-{}-{}",
            std::process::id(),
            N.fetch_add(1, O::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Config {
            dir,
            mode,
            wait,
            on_answer: None,
            mr_strip: String::new(),
            session: None,
        }
    }

    struct Client {
        w: UnixStream,
        r: BufReader<UnixStream>,
        next: u64,
        seen: Vec<Value>,
    }

    impl Client {
        fn start(c: &Config) -> (Client, JoinHandle<()>) {
            let (a, b) = UnixStream::pair().unwrap();
            let c = c.clone();
            let srv = std::thread::spawn(move || {
                serve_with(
                    BufReader::new(a.try_clone().unwrap()),
                    a,
                    &c,
                    Duration::from_millis(100),
                    "repo".into(),
                )
                .unwrap();
            });
            let mut cl = Client {
                r: BufReader::new(b.try_clone().unwrap()),
                w: b,
                next: 0,
                seen: Vec::new(),
            };
            let init = cl.call("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test-client", "version": "1"}}));
            assert!(init["result"]["instructions"].is_string());
            cl.notify("notifications/initialized", json!({}));
            (cl, srv)
        }

        fn send(&mut self, v: Value) {
            writeln!(self.w, "{v}").unwrap();
        }

        fn notify(&mut self, method: &str, params: Value) {
            self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
        }

        fn request(&mut self, method: &str, params: Value) -> u64 {
            self.next += 1;
            let id = self.next;
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
            id
        }

        fn response(&mut self, id: u64) -> Value {
            loop {
                let mut line = String::new();
                assert!(self.r.read_line(&mut line).unwrap() > 0, "server closed");
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["id"] == json!(id) {
                    return v;
                }
                self.seen.push(v);
            }
        }

        fn call(&mut self, method: &str, params: Value) -> Value {
            let id = self.request(method, params);
            self.response(id)
        }

        fn tool(&mut self, name: &str, args: Value) -> Value {
            self.call("tools/call", json!({"name": name, "arguments": args}))["result"].clone()
        }
    }

    fn text(result: &Value) -> String {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn ask(cl: &mut Client) -> Value {
        cl.tool("ask_user", json!({"question": "Ship it?", "options": ["Yes", "No"], "recommended": 1, "context": "ctx", "ticket": "ACME-1"}))
    }

    #[test]
    fn initialize_gives_the_instructions_for_each_mode() {
        for (mode, want) in [
            (Mode::Always, "instead of asking in the chat"),
            (
                Mode::OnRequest,
                "If the user asks you to send your questions",
            ),
        ] {
            let (mut cl, _) = Client::start(&cfg(1, mode));
            let r = cl.call(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "x"}}),
            );
            assert!(r["result"]["instructions"].as_str().unwrap().contains(want));
            assert_eq!(r["result"]["serverInfo"]["name"], "decisions");
            assert!(r["result"]["capabilities"]["tools"].is_object());
        }
    }

    #[test]
    fn tools_list_has_the_four_tools() {
        let (mut cl, _) = Client::start(&cfg(1, Mode::Always));
        let r = cl.call("tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["ask_user", "wait_for_answer", "reply", "withdraw"]);
        assert_eq!(
            r["result"]["tools"][0]["inputSchema"]["required"],
            json!(["question", "options"])
        );
    }

    #[test]
    fn an_answer_within_the_wait_comes_back() {
        let c = cfg(10, Mode::Always);
        let store = c.store();
        let (mut cl, _) = Client::start(&c);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            store.answer("D-1", 2, "later").unwrap();
        });
        let r = ask(&mut cl);
        assert_eq!(text(&r), "answered: 2 — No\nnote: later");
        assert_eq!(r["structuredContent"]["status"], "answered");
        assert_eq!(r["structuredContent"]["option"], 2);
        assert!(r["isError"].as_bool() != Some(true));
    }

    #[test]
    fn past_the_wait_it_is_pending_and_the_question_stays() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        let r = ask(&mut cl);
        assert!(
            text(&r).starts_with("pending: D-1 is still open"),
            "{}",
            text(&r)
        );
        assert_eq!(r["structuredContent"]["id"], "D-1");
        let d = &c.store().list().0[0];
        assert_eq!(
            (d.key.as_str(), d.source.as_str(), d.recommended),
            ("ACME-1", "test-client · repo", Some(1))
        );
    }

    #[test]
    fn asking_the_same_question_again_returns_the_same_id() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        ask(&mut cl);
        let r = ask(&mut cl);
        assert_eq!(r["structuredContent"]["id"], "D-1");
        assert_eq!(c.store().count(), 1);
    }

    #[test]
    fn a_later_server_picks_up_the_answer() {
        let c = cfg(1, Mode::Always);
        {
            let (mut cl, srv) = Client::start(&c);
            ask(&mut cl);
            drop(cl);
            srv.join().unwrap();
        }
        c.store().tell("D-1", "close it").unwrap();
        let (mut cl, _) = Client::start(&c);
        let r = cl.tool("wait_for_answer", json!({"id": "D-1"}));
        assert_eq!(text(&r), "answered in your own words: close it");
        assert_eq!(r["structuredContent"]["status"], "answered_own_words");
    }

    #[test]
    fn an_ask_back_is_reported_once_and_cleared_by_reply() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        ask(&mut cl);
        c.store().ask("D-1", "which run?").unwrap();
        let r = cl.tool("wait_for_answer", json!({"id": "D-1"}));
        assert!(
            text(&r).starts_with("the user asked back: which run?"),
            "{}",
            text(&r)
        );
        let again = cl.tool("wait_for_answer", json!({"id": "D-1"}));
        assert!(
            text(&again).starts_with("pending"),
            "not reported twice: {}",
            text(&again)
        );
        let ok = cl.tool("reply", json!({"id": "D-1", "text": "the staging one"}));
        assert!(text(&ok).contains("wait_for_answer"));
        assert!(
            c.store()
                .show("D-1")
                .unwrap()
                .contains("session: the staging one")
        );
    }

    #[test]
    fn withdraw_closes_the_question() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        ask(&mut cl);
        cl.tool("withdraw", json!({"id": "D-1", "reason": "not needed"}));
        let r = cl.tool("wait_for_answer", json!({"id": "D-1"}));
        assert_eq!(text(&r), "withdrawn: D-1 (not needed)");
        assert_eq!(c.store().count(), 0);
    }

    #[test]
    fn bad_arguments_are_tool_errors() {
        let (mut cl, _) = Client::start(&cfg(1, Mode::Always));
        let r = cl.tool("ask_user", json!({"question": "Q?", "options": ["only"]}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains("2 to 8 options"));
        let r = cl.tool("wait_for_answer", json!({"id": "D-404"}));
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn a_ticket_that_is_not_a_key_goes_into_the_context() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        cl.tool(
            "ask_user",
            json!({"question": "Q?", "options": ["a", "b"], "ticket": "#42", "context": "body"}),
        );
        let d = &c.store().list().0[0];
        assert_eq!(d.key, "");
        assert!(d.context.starts_with("Ticket: #42"), "{}", d.context);
    }

    #[test]
    fn progress_is_sent_only_with_a_token() {
        let c = cfg(1, Mode::Always);
        let (mut cl, _) = Client::start(&c);
        cl.call(
            "tools/call",
            json!({"name": "ask_user", "arguments": {"question": "Q?", "options": ["a", "b"]}}),
        );
        assert!(
            cl.seen
                .iter()
                .all(|m| m["method"] != "notifications/progress")
        );
        cl.call("tools/call", json!({"name": "wait_for_answer", "arguments": {"id": "D-1"}, "_meta": {"progressToken": "t1"}}));
        let pings: Vec<&Value> = cl
            .seen
            .iter()
            .filter(|m| m["method"] == "notifications/progress")
            .collect();
        assert!(pings.len() >= 3, "{pings:?}");
        assert_eq!(pings[0]["params"]["progressToken"], "t1");
        assert!(
            pings
                .windows(2)
                .all(|w| w[0]["params"]["progress"].as_u64() < w[1]["params"]["progress"].as_u64())
        );
    }

    #[test]
    fn the_server_ends_when_the_client_goes_away() {
        let c = cfg(0, Mode::Always);
        let (mut cl, srv) = Client::start(&c);
        cl.request(
            "tools/call",
            json!({"name": "ask_user", "arguments": {"question": "Q?", "options": ["a", "b"]}}),
        );
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        drop(cl);
        srv.join().unwrap();
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn ping_and_unknown_methods() {
        let (mut cl, _) = Client::start(&cfg(1, Mode::Always));
        assert_eq!(cl.call("ping", json!({}))["result"], json!({}));
        assert_eq!(cl.call("nope/nope", json!({}))["error"]["code"], -32601);
    }
}
