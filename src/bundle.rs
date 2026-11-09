pub struct Rec {
    pub name: String,
    pub content: String,
}

/// Strips markdown/list decoration characters from the edges of a candidate line.
fn marker_trim(s: &str) -> &str {
    s.trim_matches(|c| {
        matches!(c, '*' | '_' | '(' | ')' | '-' | '+' | '~' | '[' | ']' | '`' | ':')
    })
    .trim()
}

/// Strips a leading numbered-list prefix such as `1. ` or `2) ` from a line.
/// Requires whitespace after the delimiter so that paths like `3.14/pi.txt`
/// are left untouched.
fn strip_number_prefix(s: &str) -> &str {
    let mut out = s.trim_start();

    loop {
        let digits = out.len() - out.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            break;
        }
        let rest = &out[digits..];
        let after = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'));
        match after {
            Some(r) if r.starts_with(|c: char| c.is_whitespace()) => {
                out = r.trim_start();
            }
            _ => break,
        }
    }

    out
}

/// Validates whether a single whitespace-free token looks like a legitimate file path
/// rather than a prose word, sentence-ending period, or URL/flag.
fn is_valid_path_token(s: &str) -> bool {
    if s.is_empty() || s.len() > 160 || s.contains(char::is_whitespace) {
        return false;
    }

    // Reject URLs and CLI flags
    if s.contains("://") || s.starts_with("--") {
        return false;
    }

    // Reject characters illegal in Windows/POSIX relative paths or indicative of prose/code
    if s.chars().any(|c| {
        matches!(
            c,
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '`' | '\'' | ';' | ',' | '!' | '[' | ']' | '(' | ')' | '{' | '}'
        )
    }) {
        return false;
    }

    // A trailing dot (e.g. end of a sentence "Cli.") is never a valid filename
    if s.ends_with('.') || s.ends_with('/') || s.ends_with('\\') {
        return false;
    }

    let has_sep = s.contains('/') || s.contains('\\');

    // Check if the final path segment has a plausible extension or dotfile name
    let file_part = s.rsplit(['/', '\\']).next().unwrap_or(s);

    if let Some(dot_pos) = file_part.rfind('.') {
        let ext = &file_part[dot_pos + 1..];
        // Extension must be 1..=12 ASCII alphanumeric/+-_ chars (e.g. .rs, .toml, .ps1, .c++)
        let valid_ext = !ext.is_empty()
            && ext.len() <= 12
            && ext.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'));
        if !valid_ext {
            return false;
        }
        // Require at least one char before the dot, OR a valid dotfile like `.gitignore`
        let stem = &file_part[..dot_pos];
        if stem.is_empty() && !file_part.starts_with('.') {
            return false;
        }
        true
    } else {
        // No dot in the final segment: only accept if it has an explicit directory separator
        // (e.g. `src/Makefile` or `bin/clipout`) and valid path characters.
        has_sep
            && file_part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    }
}

/// Extracts a filename from a single candidate header line, if and only if
/// the line is formatted as a file label and not a prose sentence.
fn extract_filename_from_line(raw_line: &str) -> Option<String> {
    let mut cleaned = raw_line.trim();
    if cleaned.is_empty() {
        return None;
    }

    // Reject horizontal rules immediately
    if cleaned.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) && cleaned.len() >= 3 {
        return None;
    }

    loop {
        let prev = cleaned;
        cleaned = cleaned
            .trim_start_matches('#')
            .trim_start_matches("//")
            .trim_start_matches("<!--")
            .trim_start_matches("/*")
            .trim_end_matches("-->")
            .trim_end_matches("*/")
            .trim();

        let lower = cleaned.to_ascii_lowercase();
        if lower.starts_with("file:") {
            cleaned = cleaned[5..].trim_start_matches(':').trim();
        }

        if cleaned == prev {
            break;
        }
    }

    let cleaned = marker_trim(cleaned);
    let cleaned = marker_trim(strip_number_prefix(cleaned));

    // Case 1: Single token on the line (e.g. `Cargo.toml`, `**src/main.rs**`, `1. path/filename.rs`)
    if !cleaned.contains(char::is_whitespace) {
        if is_valid_path_token(cleaned) {
            return Some(cleaned.to_string());
        }
        return None;
    }

    // Case 2: Token followed by an explicit description separator (e.g. `2) src/lib.rs - the library`)
    if let Some(space_idx) = cleaned.find(char::is_whitespace) {
        let first_word = marker_trim(&cleaned[..space_idx]);
        let rest = cleaned[space_idx..].trim_start();

        // Only allow trailing text if it starts with a separator (`-`, `—`, `–`, `:`, `(`)
        // so that prose sentences starting with a path-like word are not misparsed,
        // while `2) src/lib.rs - the library` works as expected.
        let has_desc_separator = rest.starts_with('-')
            || rest.starts_with('—')
            || rest.starts_with('–')
            || rest.starts_with('(')
            || rest.starts_with(':');

        if has_desc_separator && is_valid_path_token(first_word) {
            return Some(first_word.to_string());
        }
    }

    None
}

pub fn find_filename_before_block(lines: &[&str], fence_index: usize) -> Option<String> {
    let mut k = fence_index;
    let mut non_empty_seen = 0usize;

    while k > 0 {
        k -= 1;
        let raw = lines[k].trim();

        if raw.is_empty() {
            // Allow at most 1 blank line between a filename header and its code fence
            if non_empty_seen > 0 {
                break;
            }
            continue;
        }

        // Stop immediately if we hit an earlier code fence or horizontal divider
        if raw.starts_with("```") || raw.starts_with("~~~") {
            break;
        }

        non_empty_seen += 1;

        // Only inspect the immediately preceding non-empty line above the fence.
        // If that line is prose (e.g. "Build and invoke with the minimal --rpc flag:"),
        // do NOT keep walking backward into earlier paragraphs!
        return extract_filename_from_line(raw);
    }

    None
}

pub fn parse_fence(text: &str, fence: &str) -> Vec<Rec> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let fence_char = fence.chars().next().unwrap_or('`');

    let mut items = Vec::new();
    let mut in_block = false;
    let mut start = 0usize;
    let mut name = String::new();
    let mut open_len = 0usize;
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if !in_block && trimmed.starts_with(fence) {
            in_block = true;
            open_len = trimmed.chars().take_while(|&c| c == fence_char).count();

            // Check if the fence info string itself contains a filename (e.g. ```rust:src/main.rs or ```src/main.rs)
            let info_str = trimmed[open_len..].trim();
            let info_filename = info_str
                .split_once(':')
                .map(|(_, rhs)| rhs.trim())
                .or_else(|| info_str.split_whitespace().find(|t| t.contains('/') || t.contains('\\') || t.contains('.')))
                .and_then(extract_filename_from_line);

            let filename = info_filename.or_else(|| find_filename_before_block(&lines, i));
            let mut n = filename.unwrap_or_default();

            if n.is_empty() {
                if i + 1 < lines.len() {
                    let mut can = lines[i + 1].trim().to_string();
                    can = can
                        .trim_start_matches("//")
                        .trim_start_matches('#')
                        .trim()
                        .replace('`', "")
                        .trim_end_matches(':')
                        .trim()
                        .to_string();
                    can = strip_number_prefix(&can).trim().to_string();

                    if is_valid_path_token(&can) {
                        n = can;
                        start = i + 2;
                    } else {
                        n = format!("file_{}.txt", items.len());
                        start = i + 1;
                    }
                } else {
                    n = format!("file_{}.txt", items.len());
                    start = i + 1;
                }
            } else {
                start = i + 1;
            }

            name = n;
            i += 1;
            continue;
        }

        if in_block {
            let run = trimmed.chars().take_while(|&c| c == fence_char).count();
            let is_pure_close = run > 0 && run == trimmed.chars().count() && run >= open_len;
            if is_pure_close {
                let content = lines[start..i].join("\r\n");
                items.push(Rec {
                    name: name.clone(),
                    content,
                });
                in_block = false;
            }
        }

        i += 1;
    }

    if in_block {
        eprintln!("Warning: bundle ended inside a fenced block; content may be truncated.");
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_numbered_list_prefixes() {
        assert_eq!(strip_number_prefix("1. path/filename"), "path/filename");
        assert_eq!(strip_number_prefix("2) path/filename.rs"), "path/filename.rs");
        assert_eq!(strip_number_prefix("12. src/main.rs"), "src/main.rs");
        assert_eq!(strip_number_prefix("1. 2. nested/path.rs"), "nested/path.rs");
    }

    #[test]
    fn keeps_paths_that_look_like_numbers() {
        assert_eq!(strip_number_prefix("3.14/pi.txt"), "3.14/pi.txt");
        assert_eq!(strip_number_prefix("2024/report.pdf"), "2024/report.pdf");
        assert_eq!(strip_number_prefix("1.5 release notes.md"), "1.5 release notes.md");
    }

    fn parse(text: &str) -> Vec<Rec> {
        parse_fence(text, "```")
    }

    #[test]
    fn numbered_filename_line_before_fence() {
        let text = "1. path/filename.rs\n```rust\nfn main() {}\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "path/filename.rs");
        assert_eq!(items[0].content, "fn main() {}");
    }

    #[test]
    fn numbered_filename_with_trailing_description() {
        let text = "2) src/lib.rs - the library\n```\npub fn x() {}\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "src/lib.rs");
    }

    #[test]
    fn multiple_numbered_files_in_bundle() {
        let text = "1. src/a.rs\n```\nA\n```\n\n2. src/b.rs\n```\nB\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "src/a.rs");
        assert_eq!(items[0].content, "A");
        assert_eq!(items[1].name, "src/b.rs");
        assert_eq!(items[1].content, "B");
    }

    #[test]
    fn numbered_bold_filename_and_file_label_still_work() {
        let text = "**3. docs/readme.md**\n```\n# Hi\n```\n\nFile: 4. notes/todo.txt\n```\nstuff\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "docs/readme.md");
        assert_eq!(items[1].name, "notes/todo.txt");
    }

    #[test]
    fn numbered_filename_inside_fence_fallback() {
        let text = "```\n1. path/filename.rs\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "path/filename.rs");
        assert_eq!(items[0].content, "");
    }

    #[test]
    fn numbered_content_line_not_eaten_when_not_path_like() {
        let text = "```\n1. step one\n2. step two\n```\n";
        let items = parse(text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "file_0.txt");
        assert_eq!(items[0].content, "1. step one\r\n2. step two");
    }

    #[test]
    fn does_not_treat_prose_sentence_or_distant_paragraph_as_filename() {
        let text = "\\*(Note on `error[E0277]` at line 106 in your compiler log: that secondary error was triggered only because `#[derive(Parser)]` failed to resolve, causing `rustc` to strip the `#[arg(...)]` attributes and fail type inference on `Cli`.)*

---

### Verification & Execution

Build and invoke with the minimal `--rpc` flag:

```powershell
cargo build --release
.\\target\\release\\curl-cli.exe https://ziggy.val.run/mcp --rpc server/discover
```
";
    let items = parse(text);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "file_0.txt");
}

}