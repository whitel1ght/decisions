//! `decisions setup <harness>`: how to register the MCP server, and how waiting behaves there.

pub const HARNESSES: [&str; 7] = [
    "claude", "codex", "gemini", "opencode", "pi", "cursor", "generic",
];

const OPTIONAL: &str = "Optional, only if the agent keeps asking in the chat (most clients show the server's own\ninstructions, so this is rarely needed). Add to AGENTS.md / CLAUDE.md:\n\n  When you need a decision, approval or clarification from me, call the decisions\n  server's ask_user tool instead of asking in the chat, and keep working on what does\n  not depend on the answer.\n";

pub fn text(harness: &str) -> Option<String> {
    let body = match harness {
        "claude" => {
            "Claude Code — register for every project:\n\n  claude mcp add --scope user decisions -e DECISIONS_WAIT=0 -- decisions mcp\n\nWaiting: Claude Code waits up to about 28 hours for a tool call and moves it to the\nbackground after 2 minutes, so DECISIONS_WAIT=0 (no limit) is right: the agent keeps\nworking and gets the answer when you give it.\n"
        }
        "codex" => {
            "Codex — add to ~/.codex/config.toml:\n\n  [mcp_servers.decisions]\n  command = \"decisions\"\n  args = [\"mcp\"]\n  tool_timeout_sec = 3600\n  env = { DECISIONS_WAIT = \"3000\" }\n\nWaiting: Codex stops a tool call after tool_timeout_sec (60 by default) and ignores progress\npings. With the values above one call waits up to 50 minutes; after that the agent gets\n\"pending\" and calls wait_for_answer again.\n"
        }
        "gemini" => {
            "Gemini CLI — add to ~/.gemini/settings.json:\n\n  \"mcpServers\": {\n    \"decisions\": { \"command\": \"decisions\", \"args\": [\"mcp\"], \"timeout\": 3600000,\n                   \"env\": { \"DECISIONS_WAIT\": \"3000\" } }\n  }\n\nWaiting: Gemini CLI stops a tool call after `timeout` ms (10 minutes by default) and ignores\nprogress pings; past DECISIONS_WAIT the agent gets \"pending\" and calls wait_for_answer.\n"
        }
        "opencode" => {
            "opencode — add to opencode.json:\n\n  \"mcp\": {\n    \"decisions\": { \"type\": \"local\", \"command\": [\"decisions\", \"mcp\"],\n                   \"environment\": { \"DECISIONS_WAIT\": \"0\" } }\n  }\n\nWaiting: opencode waits up to 12 hours by default, so DECISIONS_WAIT=0 (no limit) is fine.\n"
        }
        "pi" => {
            "pi — add to ~/.pi/agent/mcp.json:\n\n  \"mcpServers\": {\n    \"decisions\": { \"command\": \"decisions\", \"args\": [\"mcp\"], \"timeout\": 3600,\n                   \"env\": { \"DECISIONS_WAIT\": \"0\" } }\n  }\n\nWaiting: pi resets its timeout on progress pings, which the server sends every 15 s, so\nDECISIONS_WAIT=0 (no limit) works.\n"
        }
        "cursor" => {
            "Cursor — add to ~/.cursor/mcp.json:\n\n  \"mcpServers\": { \"decisions\": { \"command\": \"decisions\", \"args\": [\"mcp\"] } }\n\nWaiting: Cursor stops a tool call after about 60 s and that cannot be raised, so keep the\ndefault DECISIONS_WAIT (50): an unanswered question comes back as \"pending\" and the agent\ncalls wait_for_answer when it needs the answer.\n"
        }
        "generic" => {
            "Any MCP client — register a stdio server:\n\n  command: decisions\n  args:    [\"mcp\"]\n\nWaiting: set DECISIONS_WAIT a little below the client's tool timeout (0 = no limit, only if\nthe client waits for ever or resets its timer on progress pings). Past it the agent gets\n\"pending\" and calls wait_for_answer again; nothing is lost.\n"
        }
        _ => return None,
    };
    Some(format!(
        "The server is `decisions mcp`, a stdio MCP server.\n\n{body}\n{OPTIONAL}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_has_text_naming_the_command_and_the_optional_paragraph() {
        for h in HARNESSES {
            let t = text(h).unwrap();
            assert!(
                t.contains("decisions") && t.contains("mcp") && t.contains("Optional"),
                "{h}"
            );
        }
        assert!(text("nope").is_none());
    }
}
