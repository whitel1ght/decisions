//! The command line: the deck when no command is given, the MCP server, and the queue
//! commands scripts use (a port of the Python `decide`, same arguments and output).

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::store::{Event, New, NewAnswer, NewPrompt, Outcome};
use crate::{hook, mcp, sessions, setup, stats, tui};

#[derive(Parser)]
#[command(
    name = "decisions",
    version,
    about = "A queue of the questions your coding agents are waiting on, answered as a deck of cards"
)]
pub struct Cli {
    /// Open the deck on a made-up queue held in memory; nothing is read or written
    #[arg(long)]
    pub demo: bool,
    /// Open the deck with the prompt line open, to message a running session
    #[arg(long)]
    pub prompt: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// MCP server over stdio, for any agent harness
    Mcp,
    /// File a question (context on stdin); prints its id
    Add {
        #[arg(long, default_value = "")]
        key: String,
        #[arg(long, default_value = "")]
        mr: String,
        #[arg(long, default_value = "")]
        blocks: String,
        #[arg(long, default_value = "")]
        source: String,
        #[arg(long)]
        question: String,
        #[arg(long = "option", required = true)]
        options: Vec<String>,
        #[arg(long)]
        recommend: Option<usize>,
    },
    /// File an answer to the user's question as a card (the answer on stdin); prints its id
    Note {
        #[arg(long, default_value = "")]
        key: String,
        #[arg(long, default_value = "")]
        mr: String,
        #[arg(long, default_value = "")]
        source: String,
        #[arg(long)]
        question: String,
    },
    /// Open decisions, blocking first, then oldest first
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print a decision's file
    Show { id: String },
    /// How many questions are open (nothing when none); --badge adds the answer cards
    #[command(name = "count")]
    Count {
        #[arg(long)]
        badge: bool,
    },
    /// How the queue has been used: per-day counts and time to answer
    Stats {
        /// Days in the window, counting today
        #[arg(long, default_value_t = 14)]
        days: u32,
        #[arg(long)]
        json: bool,
    },
    /// Pick option N
    Answer {
        id: String,
        n: u8,
        note: Vec<String>,
    },
    /// Answer in your own words, none of the options
    Tell {
        id: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Ask the asker a question back
    Ask {
        id: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// The asker answers a question asked back
    Reply {
        id: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// The user has read an answer card: file it under done/<day>/
    Seen { id: String },
    /// No longer needed
    Withdraw { id: String, reason: Vec<String> },
    /// File an answered or withdrawn decision under done/<day>/
    Close { id: String },
    /// Block until something happens to a decision (exit 0 answered, 3 pending, 4 withdrawn, 5 asked back)
    Wait {
        id: String,
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// The running sessions a prompt can reach
    Sessions {
        #[arg(long)]
        json: bool,
    },
    /// Send a running session a message (its name, directory or id); prints the card its reply lands on
    Prompt {
        session: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// How to register the MCP server in a harness: claude, codex, gemini, opencode, pi, cursor, generic
    Setup { harness: String },
    /// Send open decisions to your Telegram chat while you're away, and answer from there
    Telegram {
        #[command(subcommand)]
        cmd: crate::telegram::TgCmd,
    },
}

fn refused(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("decisions: {e}");
    ExitCode::from(1)
}

fn told(cfg: &Config, r: crate::store::Result<Event>) -> ExitCode {
    match r {
        Ok(event) => {
            if let Err(w) = hook::run(cfg.on_answer.as_deref(), &event) {
                eprintln!("decisions: warning: {w}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => refused(e),
    }
}

pub fn run(cli: Cli, cfg: &Config) -> ExitCode {
    let store = cfg.store();
    let Some(cmd) = cli.cmd else {
        return tui::run(cfg, cli.demo, cli.prompt);
    };
    match cmd {
        Cmd::Mcp => match mcp::serve(std::io::stdin().lock(), std::io::stdout(), cfg) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => refused(e),
        },
        Cmd::Add {
            key,
            mr,
            blocks,
            source,
            question,
            options,
            recommend,
        } => {
            let mut context = String::new();
            if !std::io::stdin().is_terminal() {
                let _ = std::io::stdin().read_to_string(&mut context);
            }
            match store.add(New {
                key,
                mr,
                blocks,
                source,
                question,
                options,
                recommend,
                context,
            }) {
                Ok(id) => {
                    println!("{id}");
                    ExitCode::SUCCESS
                }
                Err(e) => refused(e),
            }
        }
        Cmd::Note {
            key,
            mr,
            source,
            question,
        } => {
            let mut answer = String::new();
            if !std::io::stdin().is_terminal() {
                let _ = std::io::stdin().read_to_string(&mut answer);
            }
            match store.add_answer(NewAnswer {
                key,
                mr,
                source,
                question,
                answer,
                session: String::new(),
            }) {
                Ok(id) => {
                    println!("{id}");
                    ExitCode::SUCCESS
                }
                Err(e) => refused(e),
            }
        }
        Cmd::List { json } => {
            for s in &store.skipped() {
                eprintln!("decisions: {s}");
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&store.list_json()).unwrap_or_default()
                );
            } else {
                for line in store.list_lines() {
                    println!("{line}");
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Stats { days, json } => {
            let (cards, skipped) = store.cards();
            for s in &skipped {
                eprintln!("decisions: {s}");
            }
            let stats = stats::compute(&cards, chrono::Local::now().fixed_offset(), days);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stats).unwrap_or_default()
                );
            } else {
                for line in stats::lines(&stats) {
                    println!("{line}");
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Show { id } => match store.show(&id) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => refused(e),
        },
        Cmd::Count { badge } => {
            let n = store.count();
            let answers = if badge { store.count_answers() } else { 0 };
            let line = match (badge, n, answers) {
                (false, 0, _) | (true, 0, 0) => None,
                (false, n, _) => Some(n.to_string()),
                (true, 0, a) => Some(format!("{} {a}", cfg.icons.answer)),
                (true, n, 0) => Some(format!("{} {n}", cfg.badge)),
                (true, n, a) => Some(format!("{} {n} {} {a}", cfg.badge, cfg.icons.answer)),
            };
            if let Some(line) = line {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Cmd::Seen { id } => match store.seen(&id) {
            Ok(dest) => {
                println!("{}", dest.display());
                ExitCode::SUCCESS
            }
            Err(e) => refused(e),
        },
        Cmd::Answer { id, n, note } => told(cfg, store.answer(&id, n, &note.join(" "))),
        Cmd::Tell { id, text } => told(cfg, store.tell(&id, &text.join(" "))),
        Cmd::Ask { id, text } => told(cfg, store.ask(&id, &text.join(" "))),
        Cmd::Reply { id, text } => store
            .reply(&id, &text.join(" "))
            .map_or_else(refused, |_| ExitCode::SUCCESS),
        Cmd::Withdraw { id, reason } => store
            .withdraw(&id, &reason.join(" "))
            .map_or_else(refused, |_| ExitCode::SUCCESS),
        Cmd::Close { id } => match store.close(&id) {
            Ok(dest) => {
                println!("{}", dest.display());
                ExitCode::SUCCESS
            }
            Err(e) => refused(e),
        },
        Cmd::Wait { id, timeout } => {
            let deadline = timeout.map(|s| Instant::now() + Duration::from_secs(s));
            loop {
                match store.outcome(&id) {
                    Err(e) => return refused(e),
                    Ok(Outcome::Pending) if deadline.is_none_or(|d| Instant::now() < d) => {
                        thread::sleep(Duration::from_millis(250))
                    }
                    Ok(o) => {
                        println!("{}", o.describe(&id));
                        return ExitCode::from(o.exit_code());
                    }
                }
            }
        }
        Cmd::Sessions { json } => {
            let list = sessions::running(&cfg.dir);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&list).unwrap_or_default()
                );
            } else {
                for s in &list {
                    println!(
                        "{} · {} · {} · {} · {}",
                        s.label(),
                        s.client,
                        s.dir_name(),
                        if s.state.is_empty() { "-" } else { &s.state },
                        s.owner
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Prompt { session, text } => {
            let list = sessions::running(&cfg.dir);
            let sent = sessions::resolve(&list, &session).and_then(|s| {
                store
                    .add_prompt(NewPrompt {
                        session: s.owner.clone(),
                        source: format!("prompt · {}", s.label()),
                        text: text.join(" "),
                    })
                    .map_err(|e| e.0)
            });
            match sent {
                Ok(id) => {
                    println!("{id}");
                    ExitCode::SUCCESS
                }
                Err(e) => refused(e),
            }
        }
        Cmd::Telegram { cmd } => crate::telegram::run(cmd, cfg),
        Cmd::Setup { harness } => match setup::text(&harness) {
            Some(t) => {
                print!("{t}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!(
                    "decisions: unknown harness {harness}; one of {}",
                    setup::HARNESSES.join(", ")
                );
                ExitCode::from(2)
            }
        },
    }
}
