//! What the prompt line needs from outside the deck: a session's files for `@`, images from the
//! clipboard or a pasted path. The session reads both from disk, so they go as paths.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_FILES: usize = 50_000;
const MAX_WALK_DEPTH: usize = 6;
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "__pycache__",
];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
const MAX_LISTED: usize = 5_000;

/// An image attached to the prompt line.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub path: PathBuf,
    pub bytes: u64,
}

/// The text a session receives: what was typed, then each image's path.
pub fn compose(text: &str, images: &[Image]) -> String {
    let mut parts = vec![text.trim().to_string()];
    parts.extend(
        images
            .iter()
            .map(|i| format!("[image: {}]", i.path.display())),
    );
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

fn git_files(cwd: &str) -> Option<Vec<String>> {
    let out = Command::new("git")
        .args([
            "-C",
            cwd,
            "ls-files",
            // Without -z git quotes and escapes any name that is not plain ASCII.
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .take(MAX_FILES)
            .map(String::from)
            .collect()
    })
}

/// Outside git: every file that is not hidden or a dependency, nearest first, so the limit cuts
/// the deepest ones.
fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([(PathBuf::new(), 0)]);
    while let Some((rel, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let text = name.to_string_lossy();
            if text.starts_with('.') || SKIPPED_DIRS.contains(&text.as_ref()) {
                continue;
            }
            let path = rel.join(&name);
            match e.file_type() {
                Ok(t) if t.is_dir() && depth < MAX_WALK_DEPTH => queue.push_back((path, depth + 1)),
                Ok(t) if t.is_file() && out.len() < MAX_FILES => {
                    out.push(path.to_string_lossy().into_owned())
                }
                _ => {}
            }
        }
    }
    out
}

/// The files `@` offers for a session in `cwd`, with their directories: what git knows and does
/// not ignore, else a walk.
pub fn files(cwd: &str) -> Vec<String> {
    let mut list = git_files(cwd).unwrap_or_else(|| walk(Path::new(cwd)));
    let mut dirs: Vec<String> = list
        .iter()
        .flat_map(|f| {
            Path::new(f)
                .ancestors()
                .skip(1)
                .filter(|a| !a.as_os_str().is_empty())
                .map(|a| format!("{}/", a.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    list.append(&mut dirs);
    list
}

/// How well `path` matches `query`, both lower case: lower is better, None is no match.
fn score(path: &str, query: &str) -> Option<usize> {
    let trimmed = path.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if base.starts_with(query) {
        return Some(0);
    }
    if base.contains(query) {
        return Some(1);
    }
    if path.contains(query) {
        return Some(2);
    }
    let mut chars = path.chars();
    query.chars().all(|q| chars.any(|c| c == q)).then_some(3)
}

/// The best `limit` of `files` for `query`, closest and shortest first. A line range after the
/// path (`#L10-20`) takes no part in the search.
pub fn search(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let query = query.split('#').next().unwrap_or(query).to_lowercase();
    let mut found: Vec<(usize, &String)> = files
        .iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            // A directory just picked would come first and picking it again would do nothing.
            if lower == query && lower.ends_with('/') {
                return None;
            }
            score(&lower, &query).map(|s| (s, f))
        })
        .collect();
    found.sort_by_key(|(s, f)| (*s, f.len(), f.as_str()));
    found
        .into_iter()
        .take(limit)
        .map(|(_, f)| f.clone())
        .collect()
}

/// What follows `@` when it names a path by where it is rather than searching the session's
/// files: the folder as typed, and the start of a name in it. A line range takes no part.
pub fn outside_path(query: &str) -> Option<(&str, &str)> {
    let path = query.split('#').next().unwrap_or(query);
    if !["/", "~/", "./", "../"].iter().any(|p| path.starts_with(p)) {
        return None;
    }
    let cut = path.rfind('/')? + 1;
    Some(path.split_at(cut))
}

/// What is in folder `dir`, written as `dir` was, so `~/` stays `~/` in the message. A folder ends
/// in `/`. A relative `dir` is from `cwd`, the session's directory, where the session will read it.
pub fn list_dir(cwd: &str, dir: &str) -> Vec<String> {
    let folder = expand_home(dir.to_string());
    let folder = if folder.is_absolute() {
        folder
    } else {
        Path::new(cwd).join(folder)
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut list: Vec<String> = entries
        .flatten()
        .take(MAX_LISTED)
        .map(|e| {
            let slash = if e.path().is_dir() { "/" } else { "" };
            format!("{dir}{}{slash}", e.file_name().to_string_lossy())
        })
        .collect();
    list.sort_by_key(|p| p.to_lowercase());
    list
}

/// The best `limit` of a folder's entries for the start of a name: names starting with it, then
/// names containing it, each in folder order. Hidden ones only once a dot is typed.
pub fn path_rows(entries: &[String], dir: &str, name: &str, limit: usize) -> Vec<String> {
    let name = name.to_lowercase();
    let mut found: Vec<(usize, &String)> = entries
        .iter()
        .filter_map(|e| {
            let base = e.strip_prefix(dir)?.trim_end_matches('/').to_lowercase();
            if base.starts_with('.') && !name.starts_with('.') {
                return None;
            }
            if base.starts_with(&name) {
                Some((0, e))
            } else {
                base.contains(&name).then_some((1, e))
            }
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found
        .into_iter()
        .take(limit)
        .map(|(_, e)| e.clone())
        .collect()
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| IMAGE_EXTENSIONS.contains(&x.to_lowercase().as_str()))
}

fn expand_home(path: String) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// Reads a paste as a shell would, since that is how a terminal escapes a dragged path: quotes and
/// backslashes hold a name together. Only ASCII whitespace separates, because macOS puts U+202F in
/// its screenshots' names.
fn tokens(text: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut escaped) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
        } else if quote.is_none() && c.is_ascii_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The images a paste names, when it is nothing but paths to existing images: dragging a file
/// onto the terminal pastes its path.
pub fn pasted_images(text: &str) -> Option<Vec<Image>> {
    let found: Option<Vec<Image>> = tokens(text)
        .iter()
        .map(|t| {
            let path = expand_home(t.clone());
            if !is_image(&path) {
                return None;
            }
            let bytes = std::fs::metadata(&path).ok().filter(|m| m.is_file())?.len();
            Some(Image { path, bytes })
        })
        .collect();
    found.filter(|f| !f.is_empty())
}

#[cfg(target_os = "macos")]
fn clipboard_to(path: &Path) -> Result<(), String> {
    let target = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ran = Command::new("osascript")
        .args([
            "-e",
            &format!("set f to open for access POSIX file \"{target}\" with write permission"),
            "-e",
            "try",
            "-e",
            "write (the clipboard as «class PNGf») to f",
            "-e",
            "on error e",
            "-e",
            "close access f",
            "-e",
            "error e",
            "-e",
            "end try",
            "-e",
            "close access f",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if ran.success() {
        Ok(())
    } else {
        Err("no image on the clipboard".into())
    }
}

#[cfg(not(target_os = "macos"))]
fn clipboard_to(path: &Path) -> Result<(), String> {
    let tools: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--type", "image/png"]),
        (
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-o"],
        ),
    ];
    for (tool, args) in tools {
        let Ok(out) = Command::new(tool)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        if out.status.success() && !out.stdout.is_empty() {
            return std::fs::write(path, out.stdout).map_err(|e| e.to_string());
        }
    }
    Err("no image on the clipboard (needs wl-paste or xclip)".into())
}

/// Saves the clipboard's image under `<dir>/attachments/`.
pub fn paste_image(dir: &Path, stamp: i64) -> Result<Image, String> {
    let folder = dir.join("attachments");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{stamp}.png"));
    let saved = clipboard_to(&path).and_then(|()| {
        let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if bytes == 0 {
            return Err("no image on the clipboard".to_string());
        }
        Ok(Image {
            path: path.clone(),
            bytes,
        })
    });
    if saved.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("decisions-prompt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn compose_puts_the_paths_after_the_text() {
        let img = Image {
            path: "/a/1.png".into(),
            bytes: 1,
        };
        assert_eq!(
            compose(" look ", std::slice::from_ref(&img)),
            "look [image: /a/1.png]"
        );
        assert_eq!(compose("", &[img]), "[image: /a/1.png]");
        assert_eq!(compose("look", &[]), "look");
    }

    #[test]
    fn search_prefers_the_file_name_then_the_path_then_letters_in_order() {
        let files: Vec<String> = [
            "src/components/FilterBar.vue",
            "src/filters/index.ts",
            "docs/a-filter-note.md",
            "src/f_i_l.rs",
            "src/other.rs",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            search(&files, "filter", 9),
            [
                "src/components/FilterBar.vue",
                "docs/a-filter-note.md",
                "src/filters/index.ts"
            ]
        );
        assert_eq!(search(&files, "fil", 1), ["src/components/FilterBar.vue"]);
        assert!(search(&files, "srcfil", 9).contains(&"src/f_i_l.rs".to_string()));
    }

    #[test]
    fn a_path_by_where_it_is_is_told_from_a_search() {
        assert_eq!(outside_path("~/wik"), Some(("~/", "wik")));
        assert_eq!(outside_path("~/"), Some(("~/", "")));
        assert_eq!(outside_path("/Users/me/pro"), Some(("/Users/me/", "pro")));
        assert_eq!(outside_path("/"), Some(("/", "")));
        assert_eq!(outside_path("../dot"), Some(("../", "dot")));
        assert_eq!(outside_path("../../a/b"), Some(("../../a/", "b")));
        assert_eq!(outside_path("./sr"), Some(("./", "sr")));
        assert_eq!(outside_path("~/a.rs#L1-2"), Some(("~/", "a.rs")));
        for search in ["", "app", "src/app", "~", "..", ".", ".env", "~wiki"] {
            assert_eq!(outside_path(search), None, "{search}");
        }
    }

    fn folder(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("decisions-prompt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Sub")).unwrap();
        std::fs::create_dir_all(d.join("cwd")).unwrap();
        std::fs::write(d.join("zeta.md"), "").unwrap();
        std::fs::write(d.join("alpha.md"), "").unwrap();
        std::fs::write(d.join(".hidden"), "").unwrap();
        d
    }

    #[test]
    fn a_folder_lists_in_the_form_it_was_typed() {
        let d = folder("forms");
        let abs = format!("{}/", d.display());
        let names = |list: Vec<String>, dir: &str| -> Vec<String> {
            list.iter()
                .map(|e| e.strip_prefix(dir).unwrap().to_string())
                .collect()
        };
        assert_eq!(
            names(list_dir("", &abs), &abs),
            [".hidden", "alpha.md", "cwd/", "Sub/", "zeta.md"]
        );
        let cwd = d.join("cwd");
        assert_eq!(
            names(list_dir(&cwd.to_string_lossy(), "../"), "../"),
            [".hidden", "alpha.md", "cwd/", "Sub/", "zeta.md"]
        );
        assert!(list_dir(&cwd.to_string_lossy(), "./").is_empty());
        assert!(list_dir("", "/no/such/folder/").is_empty());
    }

    #[test]
    fn home_is_expanded_only_to_read_it() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let want = std::fs::read_dir(&home).unwrap().count();
        let got = list_dir("/", "~/");
        assert_eq!(got.len(), want.min(MAX_LISTED));
        assert!(got.iter().all(|e| e.starts_with("~/")), "{got:?}");
    }

    #[test]
    fn a_link_to_a_folder_is_a_folder() {
        let d = folder("link");
        std::os::unix::fs::symlink(d.join("Sub"), d.join("linked")).unwrap();
        let dir = format!("{}/", d.display());
        assert!(list_dir("", &dir).contains(&format!("{dir}linked/")));
    }

    #[test]
    fn a_folders_rows_start_with_the_name_then_contain_it_and_hide_dotfiles() {
        let list: Vec<String> = ["~/.wiki", "~/awiki.md", "~/wiki/", "~/Wikipedia", "~/zz"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            path_rows(&list, "~/", "wik", 8),
            ["~/wiki/", "~/Wikipedia", "~/awiki.md"]
        );
        assert_eq!(path_rows(&list, "~/", "", 2), ["~/awiki.md", "~/wiki/"]);
        assert_eq!(path_rows(&list, "~/", ".w", 8), ["~/.wiki"]);
        assert_eq!(path_rows(&list, "~/", "nope", 8), Vec::<String>::new());
    }

    #[test]
    fn search_ignores_a_line_range() {
        let files = vec!["src/app.rs".to_string()];
        assert_eq!(search(&files, "app.rs#L10-20", 9), files);
    }

    #[test]
    fn files_lists_what_is_there_with_directories_outside_git() {
        let d = temp("walk");
        std::fs::create_dir_all(d.join("src/deep")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        std::fs::create_dir_all(d.join("web/node_modules/lib")).unwrap();
        std::fs::write(d.join("src/deep/a.rs"), "").unwrap();
        std::fs::write(d.join(".hidden/b.rs"), "").unwrap();
        std::fs::write(d.join("web/node_modules/lib/hidden.js"), "").unwrap();
        let got = files(d.to_str().unwrap());
        assert!(got.contains(&"src/deep/a.rs".to_string()), "{got:?}");
        assert!(got.contains(&"src/deep/".to_string()), "{got:?}");
        assert!(got.contains(&"src/".to_string()), "{got:?}");
        assert!(!got.iter().any(|f| f.contains("hidden")), "{got:?}");
    }

    #[test]
    fn a_pasted_image_path_is_an_image_in_any_form_a_terminal_sends() {
        let d = temp("paste");
        let plain = d.join("one.png");
        let spaced = d.join("two words.JPG");
        std::fs::write(&plain, "12345").unwrap();
        std::fs::write(&spaced, "1").unwrap();
        let p = plain.display().to_string();
        let s = spaced.display().to_string();
        assert_eq!(pasted_images(&p).unwrap()[0].bytes, 5);
        assert_eq!(
            pasted_images(&s.replace(' ', "\\ ")).unwrap()[0].path,
            spaced
        );
        assert_eq!(pasted_images(&format!("'{s}'")).unwrap()[0].path, spaced);
        assert_eq!(pasted_images(&format!("{p} '{s}'")).unwrap().len(), 2);
    }

    #[test]
    fn a_dragged_path_with_shell_escapes_or_a_screenshots_narrow_space_is_found() {
        let d = temp("escapes");
        let copy = d.join("image (1).png");
        let shot = d.join("Screenshot at 9.41.05\u{202f}PM.png");
        std::fs::write(&copy, "1").unwrap();
        std::fs::write(&shot, "1").unwrap();
        let escaped = copy
            .display()
            .to_string()
            .replace(' ', "\\ ")
            .replace('(', "\\(")
            .replace(')', "\\)");
        assert_eq!(pasted_images(&escaped).unwrap()[0].path, copy);
        let raw = shot.display().to_string().replace(' ', "\\ ");
        assert_eq!(pasted_images(&raw).unwrap()[0].path, shot);
    }

    #[test]
    fn git_names_that_are_not_ascii_are_listed_as_they_are() {
        let d = temp("git");
        std::fs::create_dir_all(d.join("документы")).unwrap();
        std::fs::write(d.join("документы/заметка.md"), "").unwrap();
        let init = Command::new("git")
            .args(["-C", d.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap();
        assert!(init.success());
        let got = files(d.to_str().unwrap());
        assert!(got.contains(&"документы/заметка.md".to_string()), "{got:?}");
        assert!(got.contains(&"документы/".to_string()), "{got:?}");
        assert_eq!(search(&got, "заметка", 1), ["документы/заметка.md"]);
    }

    #[test]
    fn a_directory_just_picked_gives_way_to_what_is_inside_it() {
        let files: Vec<String> = ["src/", "src/app.rs"].map(String::from).to_vec();
        assert_eq!(search(&files, "src/", 9), ["src/app.rs"]);
    }

    #[test]
    fn anything_else_pasted_is_text() {
        let d = temp("text");
        let text = d.join("notes.txt");
        std::fs::write(&text, "x").unwrap();
        assert_eq!(pasted_images("just some words"), None);
        assert_eq!(pasted_images(text.to_str().unwrap()), None);
        assert_eq!(pasted_images("/no/such/file.png"), None);
        assert_eq!(pasted_images("  "), None);
    }
}
