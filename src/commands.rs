//! The commands on a card, for the deck's `c` key: fenced blocks of the context first, then
//! inline `code` spans holding a space (a command, not an identifier), in the context and the
//! options. Each is exactly what the agent wrote.

pub fn commands(context: &str, options: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let mut prose = String::new();
    let mut block: Option<Vec<&str>> = None;
    for line in context.lines() {
        if line.trim_start().starts_with("```") {
            match block.take() {
                Some(lines) => found.push(lines.join("\n")),
                None => block = Some(Vec::new()),
            }
            continue;
        }
        match block.as_mut() {
            Some(lines) => lines.push(line),
            None => {
                prose.push_str(line);
                prose.push('\n');
            }
        }
    }
    if let Some(lines) = block {
        found.push(lines.join("\n"));
    }
    for text in std::iter::once(prose.as_str()).chain(options.iter().copied()) {
        found.extend(inline(text));
    }
    let mut out: Vec<String> = Vec::new();
    for c in found {
        if !c.trim().is_empty() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Closed `code` spans that hold a space.
fn inline(text: &str) -> Vec<String> {
    let parts: Vec<&str> = text.split('`').collect();
    (1..parts.len().saturating_sub(1))
        .step_by(2)
        .map(|i| parts[i].trim())
        .filter(|span| span.contains(' '))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_blocks_come_first_joined_by_newlines() {
        let ctx = "x\n```bash\nkubectl get pods\n  -n prod\n```\nthen `git push origin main`";
        assert_eq!(
            commands(ctx, &[]),
            ["kubectl get pods\n  -n prod", "git push origin main"]
        );
    }

    #[test]
    fn inline_code_needs_a_space() {
        assert!(commands("see `D-12` on `main`", &[]).is_empty());
    }

    #[test]
    fn options_are_scanned_after_the_context() {
        let opt = "You run `duploctl jit k8s --interactive` and sign in";
        assert_eq!(
            commands("run `make test` first", &[opt]),
            ["make test", "duploctl jit k8s --interactive"]
        );
    }

    #[test]
    fn duplicates_are_kept_once() {
        assert_eq!(
            commands(
                "```\nmake test\n```\nor `make test`",
                &["You run `make test`"]
            ),
            ["make test"]
        );
    }

    #[test]
    fn unterminated_fence_runs_to_the_end() {
        assert_eq!(commands("```sh\nls -la\npwd", &[]), ["ls -la\npwd"]);
    }

    #[test]
    fn backticks_inside_a_fence_are_not_inline_code() {
        assert_eq!(
            commands("```\necho `date` now\n```", &[]),
            ["echo `date` now"]
        );
    }
}
