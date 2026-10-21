//! The Telegram Bot API over HTTPS: the only code that touches the network. Every error text
//! that leaves this file has the token replaced by `<token>`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::telegram::render::Button;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No network, a 5xx or a 429: try again, after `after` seconds when Telegram said so.
    Retry { after: Option<u64>, msg: String },
    /// 401: the token is wrong or revoked.
    Unauthorized(String),
    /// 403: the user blocked the bot.
    Forbidden(String),
    /// 400 and anything else Telegram refuses.
    Refused(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Retry { msg, .. }
            | ApiError::Unauthorized(msg)
            | ApiError::Forbidden(msg)
            | ApiError::Refused(msg) => f.write_str(msg),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Message {
        chat_id: i64,
        private: bool,
        message_id: i64,
        text: String,
        reply_to: Option<i64>,
    },
    Press {
        query_id: String,
        chat_id: i64,
        message_id: i64,
        data: String,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub id: i64,
    pub kind: Kind,
}

/// None only without an `update_id`; anything unreadable is `Other`, so its id still moves
/// the offset on.
pub fn parse_update(v: &Value) -> Option<Update> {
    let id = v["update_id"].as_i64()?;
    let message = |m: &Value| {
        Some(Kind::Message {
            chat_id: m["chat"]["id"].as_i64()?,
            private: m["chat"]["type"] == "private",
            message_id: m["message_id"].as_i64()?,
            text: m["text"].as_str().unwrap_or("").into(),
            reply_to: m["reply_to_message"]["message_id"].as_i64(),
        })
    };
    let press = |c: &Value| {
        Some(Kind::Press {
            query_id: c["id"].as_str()?.into(),
            chat_id: c["message"]["chat"]["id"].as_i64()?,
            message_id: c["message"]["message_id"].as_i64()?,
            data: c["data"].as_str().unwrap_or("").into(),
        })
    };
    let kind = if let Some(m) = v.get("message") {
        message(m)
    } else if let Some(c) = v.get("callback_query") {
        press(c)
    } else {
        None
    };
    Some(Update {
        id,
        kind: kind.unwrap_or(Kind::Other),
    })
}

pub trait Api {
    /// The bot's username.
    fn get_me(&mut self) -> Result<String, ApiError>;
    fn get_updates(&mut self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, ApiError>;
    /// The new message's id.
    fn send(
        &mut self,
        chat_id: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError>;
    /// Replaces text and buttons (no buttons removes them); "message is not modified" is Ok.
    fn edit(
        &mut self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
    ) -> Result<(), ApiError>;
    fn answer_press(&mut self, query_id: &str, text: &str) -> Result<(), ApiError>;
}

pub struct Http {
    agent: ureq::Agent,
    base: String,
    token: String,
}

fn markup(keyboard: &[Vec<Button>]) -> Value {
    let rows: Vec<Vec<Value>> = keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| json!({"text": b.text, "callback_data": b.data}))
                .collect()
        })
        .collect();
    json!({ "inline_keyboard": rows })
}

impl Http {
    pub fn new(api: &str, token: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(40)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            base: api.trim_end_matches('/').into(),
            token: token.into(),
        }
    }

    fn redact(&self, s: &str) -> String {
        if self.token.is_empty() {
            s.into()
        } else {
            s.replace(&self.token, "<token>")
        }
    }

    fn call(&self, method: &str, body: Value) -> Result<Value, ApiError> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let mut res = self
            .agent
            .post(&url)
            .send_json(&body)
            .map_err(|e| ApiError::Retry {
                after: None,
                msg: self.redact(&format!("{method}: {e}")),
            })?;
        let status = res.status().as_u16();
        let v: Value = res.body_mut().read_json().unwrap_or(Value::Null);
        if status == 200 && v["ok"] == true {
            return Ok(v["result"].clone());
        }
        let desc = v["description"].as_str().unwrap_or("no description");
        let msg = self.redact(&format!("{method}: {status} {desc}"));
        Err(match status {
            401 => ApiError::Unauthorized(msg),
            403 => ApiError::Forbidden(msg),
            429 => ApiError::Retry {
                after: v["parameters"]["retry_after"].as_u64(),
                msg,
            },
            500.. => ApiError::Retry { after: None, msg },
            _ => ApiError::Refused(msg),
        })
    }
}

impl Api for Http {
    fn get_me(&mut self) -> Result<String, ApiError> {
        Ok(self.call("getMe", json!({}))?["username"]
            .as_str()
            .unwrap_or("")
            .into())
    }

    fn get_updates(&mut self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, ApiError> {
        let r = self.call(
            "getUpdates",
            json!({"offset": offset, "timeout": timeout_secs, "allowed_updates": ["message", "callback_query"]}),
        )?;
        Ok(r.as_array()
            .map(|a| a.iter().filter_map(parse_update).collect())
            .unwrap_or_default())
    }

    fn send(
        &mut self,
        chat_id: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError> {
        let mut body = json!({"chat_id": chat_id, "text": text, "link_preview_options": {"is_disabled": true}});
        if html {
            body["parse_mode"] = json!("HTML");
        }
        if !keyboard.is_empty() {
            body["reply_markup"] = markup(keyboard);
        }
        if let Some(r) = reply_to {
            body["reply_parameters"] =
                json!({"message_id": r, "allow_sending_without_reply": true});
        }
        self.call("sendMessage", body)?["message_id"]
            .as_i64()
            .ok_or_else(|| ApiError::Refused("sendMessage: no message_id in the reply".into()))
    }

    fn edit(
        &mut self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
    ) -> Result<(), ApiError> {
        let mut body = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
            "link_preview_options": {"is_disabled": true},
            "reply_markup": markup(keyboard),
        });
        if html {
            body["parse_mode"] = json!("HTML");
        }
        match self.call("editMessageText", body) {
            Err(ApiError::Refused(m)) if m.contains("message is not modified") => Ok(()),
            r => r.map(|_| ()),
        }
    }

    fn answer_press(&mut self, query_id: &str, text: &str) -> Result<(), ApiError> {
        self.call(
            "answerCallbackQuery",
            json!({"callback_query_id": query_id, "text": text}),
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::render::Button;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    const TOKEN: &str = "123:SECRETabc";

    /// A one-shot HTTP server per response; sends back each request's path and body.
    fn serve(responses: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<(String, Value)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (code, body) in responses {
                let (mut s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut buf = vec![0; len];
                r.read_exact(&mut buf).unwrap();
                tx.send((path, serde_json::from_slice(&buf).unwrap_or(Value::Null)))
                    .unwrap();
                write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (base, rx)
    }

    #[test]
    fn send_posts_html_with_buttons_and_returns_the_id() {
        let (base, rx) = serve(vec![(200, r#"{"ok":true,"result":{"message_id":42}}"#)]);
        let kb = vec![vec![Button {
            text: "1 · Yes".into(),
            data: "a:D-1:1".into(),
        }]];
        let id = Http::new(&base, TOKEN)
            .send(7, "<b>Q</b>", true, &kb, Some(3))
            .unwrap();
        assert_eq!(id, 42);
        let (path, body) = rx.recv().unwrap();
        assert_eq!(path, format!("/bot{TOKEN}/sendMessage"));
        assert_eq!(body["chat_id"], 7);
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(
            body["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "a:D-1:1"
        );
        assert_eq!(body["reply_parameters"]["message_id"], 3);
    }

    #[test]
    fn get_updates_passes_offset_and_timeout() {
        let (base, rx) = serve(vec![(
            200,
            r#"{"ok":true,"result":[{"update_id":5,"message":{"message_id":9,"chat":{"id":7,"type":"private"},"text":"hi"}}]}"#,
        )]);
        let u = Http::new(&base, TOKEN).get_updates(5, 5).unwrap();
        assert_eq!(u[0].id, 5);
        let (_, body) = rx.recv().unwrap();
        assert_eq!(
            (body["offset"].as_i64(), body["timeout"].as_u64()),
            (Some(5), Some(5))
        );
    }

    #[test]
    fn status_codes_become_errors() {
        let (base, _rx) = serve(vec![
            (
                429,
                r#"{"ok":false,"error_code":429,"description":"Too Many Requests","parameters":{"retry_after":3}}"#,
            ),
            (
                401,
                r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#,
            ),
            (
                403,
                r#"{"ok":false,"error_code":403,"description":"Forbidden: bot was blocked by the user"}"#,
            ),
            (502, "bad gateway"),
            (
                400,
                r#"{"ok":false,"error_code":400,"description":"Bad Request: message is not modified"}"#,
            ),
            (
                400,
                r#"{"ok":false,"error_code":400,"description":"Bad Request: can't parse entities"}"#,
            ),
        ]);
        let mut h = Http::new(&base, TOKEN);
        assert!(matches!(
            h.get_me(),
            Err(ApiError::Retry { after: Some(3), .. })
        ));
        assert!(matches!(h.get_me(), Err(ApiError::Unauthorized(_))));
        assert!(matches!(h.get_me(), Err(ApiError::Forbidden(_))));
        assert!(matches!(
            h.get_me(),
            Err(ApiError::Retry { after: None, .. })
        ));
        assert_eq!(
            h.edit(7, 9, "x", false, &[]),
            Ok(()),
            "not modified counts as done"
        );
        assert!(
            matches!(h.edit(7, 9, "x", true, &[]), Err(ApiError::Refused(m)) if m.contains("parse entities"))
        );
    }

    #[test]
    fn errors_never_carry_the_token() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let e = Http::new(&base, TOKEN).get_me().unwrap_err();
        assert!(matches!(e, ApiError::Retry { .. }));
        assert!(!e.to_string().contains("SECRET"), "{e}");
        let (base, _rx) = serve(vec![(
            401,
            r#"{"ok":false,"description":"token 123:SECRETabc is bad"}"#,
        )]);
        let e = Http::new(&base, TOKEN).get_me().unwrap_err();
        assert!(
            !e.to_string().contains("SECRET") && e.to_string().contains("<token>"),
            "{e}"
        );
    }

    #[test]
    fn updates_are_parsed_and_odd_ones_still_count() {
        let reply = json!({"update_id": 1, "message": {"message_id": 9, "chat": {"id": 7, "type": "private"}, "text": "2 ok", "reply_to_message": {"message_id": 4}}});
        assert_eq!(
            parse_update(&reply),
            Some(Update {
                id: 1,
                kind: Kind::Message {
                    chat_id: 7,
                    private: true,
                    message_id: 9,
                    text: "2 ok".into(),
                    reply_to: Some(4)
                }
            })
        );
        let press = json!({"update_id": 2, "callback_query": {"id": "q", "data": "u:D-1", "message": {"message_id": 4, "chat": {"id": 7, "type": "private"}}}});
        assert_eq!(
            parse_update(&press),
            Some(Update {
                id: 2,
                kind: Kind::Press {
                    query_id: "q".into(),
                    chat_id: 7,
                    message_id: 4,
                    data: "u:D-1".into()
                }
            })
        );
        let odd = json!({"update_id": 3, "message": {"chat": {}}});
        assert_eq!(
            parse_update(&odd),
            Some(Update {
                id: 3,
                kind: Kind::Other
            })
        );
        assert_eq!(parse_update(&json!({"x": 1})), None);
    }
}
