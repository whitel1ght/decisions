//! Just enough markdown for a decision's context: headings, bullets, fenced code, inline
//! code and bold, wrapped to the card's width so the back knows how far it can scroll.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme;

pub fn render(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(10));
    let base = Style::new().fg(theme::TEXT);
    let mut out = Vec::new();
    let mut fenced = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(Line::styled(
                format!("  {line}"),
                Style::new().fg(theme::CODE),
            ));
            continue;
        }
        if line.is_empty() {
            out.push(Line::default());
            continue;
        }
        let (prefix, body, style) = if let Some(h) = heading(line) {
            (
                "",
                h,
                Style::new().fg(theme::BLUE).add_modifier(Modifier::BOLD),
            )
        } else if let Some(b) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            ("• ", b, base)
        } else {
            ("", line, base)
        };
        out.extend(wrap(prefix, inline(body, style), width));
    }
    out
}

fn heading(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches('#');
    (rest.len() < line.len() && rest.starts_with(' ')).then(|| rest.trim_start())
}

fn inline(text: &str, base: Style) -> Vec<(String, Style)> {
    let style = |bold: bool, code: bool| {
        if code {
            Style::new().fg(theme::CODE)
        } else if bold {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        }
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut code) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let toggle_bold = c == '*' && !code && chars.peek() == Some(&'*');
        if c == '`' || toggle_bold {
            if !cur.is_empty() {
                out.push((std::mem::take(&mut cur), style(bold, code)));
            }
            if toggle_bold {
                chars.next();
                bold = !bold;
            } else {
                code = !code;
            }
            continue;
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push((cur, style(bold, code)));
    }
    out
}

fn wrap(prefix: &str, spans: Vec<(String, Style)>, width: usize) -> Vec<Line<'static>> {
    let indent = " ".repeat(prefix.width());
    let mut lines = Vec::new();
    let mut line = vec![Span::raw(prefix.to_string())];
    let mut used = prefix.width();
    for (text, style) in spans {
        for word in text.split_inclusive(' ') {
            let need = word.trim_end().width();
            if used + need > width && used > indent.len() {
                lines.push(finish(std::mem::replace(
                    &mut line,
                    vec![Span::raw(indent.clone())],
                )));
                used = indent.len();
            }
            let word = if used == indent.len() {
                word.trim_start()
            } else {
                word
            };
            used += word.width();
            line.push(Span::styled(word.to_string(), style));
        }
    }
    lines.push(finish(line));
    lines
}

fn finish(mut spans: Vec<Span<'static>>) -> Line<'static> {
    spans.retain(|s| !s.content.is_empty());
    if let Some(last) = spans.last_mut() {
        last.content = last.content.trim_end().to_string().into();
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn headings_lose_their_hashes_and_turn_bold() {
        let l = render("### Why", 40);
        assert_eq!(plain(&l), ["Why"]);
        assert!(l[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(l[0].spans[0].style.fg, Some(theme::BLUE));
    }

    #[test]
    fn bullets_get_a_dot_and_wrapped_lines_indent_under_it() {
        assert_eq!(
            plain(&render("- one two three four five six", 14)),
            ["• one two", "  three four", "  five six"]
        );
    }

    #[test]
    fn inline_code_and_bold_lose_their_markers() {
        let l = render("a **two** and `x`", 40);
        assert_eq!(plain(&l), ["a two and x"]);
        let bold = l[0]
            .spans
            .iter()
            .find(|s| s.content.contains("two"))
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = l[0].spans.iter().find(|s| s.content.contains('x')).unwrap();
        assert_eq!(code.style.fg, Some(theme::CODE));
    }

    #[test]
    fn fences_are_dropped_and_their_lines_kept_as_code() {
        let l = render("```\nlet x = 1;\n```", 40);
        assert_eq!(plain(&l), ["  let x = 1;"]);
        assert_eq!(l[0].style.fg, Some(theme::CODE));
    }

    #[test]
    fn blank_lines_stay() {
        assert_eq!(plain(&render("a\n\nb", 40)), ["a", "", "b"]);
    }

    #[test]
    fn a_word_longer_than_the_width_gets_its_own_line() {
        let url = "https://gitlab.com/acme/acme-backend/-/merge_requests/870#note_4026396373";
        assert_eq!(
            plain(&render(&format!("see {url} now"), 20)),
            ["see", url, "now"]
        );
    }

    #[test]
    fn the_fixture_context_renders() {
        let ctx = "### Why\n\nThe reviewer asked for **two** paths.\n\n- `fallbackMeter` misses the DMS save\n- the retry path\n\n```\ncode line\n```";
        assert_eq!(
            plain(&render(ctx, 60)),
            [
                "Why",
                "",
                "The reviewer asked for two paths.",
                "",
                "• fallbackMeter misses the DMS save",
                "• the retry path",
                "",
                "  code line"
            ]
        );
    }
}
