pub struct Rec {
    pub name: String,
    pub content: String,
}

/// Strips markdown/list decoration characters from the edges of a candidate line.
fn marker_trim(s: &str) -> &str {
    s.trim_matches(|c| {
        matches!(c, '*' | '(' | ')' | '-' | '+' | '~' | '[' | ']' | '`' | ':')
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

pub fn find_filename_before_block(lines: &[&str], fence_index: usize) -> Option<String> {
    let mut k = fence_index;

    while k > 0 {
        k -= 1;
        let raw = lines[k].trim();

        if raw.is_empty() {
            continue;
        }

        let mut cleaned = raw;
        loop {
            let prev = cleaned;
            cleaned = cleaned.trim_start_matches('#')
                             .trim_start_matches("//")
                             .trim_start_matches("<!--")
                             .trim_start_matches("/*")
                             .trim();
            
            let lower = cleaned.to_lowercase();
            if lower.starts_with("file: ") || lower.starts_with("file:") {
                cleaned = cleaned[5..].trim_start_matches(':').trim();
            }
            
            if cleaned == prev {
                break;
            }
        }

        let cleaned = marker_trim(cleaned);
        let cleaned = marker_trim(strip_number_prefix(cleaned));

        if cleaned.starts_with('.') || cleaned.contains('.') || cleaned.contains('/') || cleaned.contains('\\') {
            if let Some(space_idx) = cleaned.find(' ') {
                let first_word = &cleaned[..space_idx];
                if first_word.starts_with('.') || first_word.contains('.') || first_word.contains('/') || first_word.contains('\\') {
                    return Some(first_word.to_string());
                }
            }
            return Some(cleaned.to_string());
        }
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

            let filename = find_filename_before_block(&lines, i);
            let mut n = filename.unwrap_or_else(String::new);

            if !n.contains('.') && !n.contains('/') && !n.contains('\\') && !n.starts_with('.') {
                if i + 1 < lines.len() {
                    let mut can = lines[i + 1].trim().to_string();
                    can = can.replace('`', "").trim_end_matches(':').trim().to_string();
                    can = strip_number_prefix(&can).trim().to_string();
                    
                    if (can.contains('.') || can.contains('/') || can.contains('\\') || can.starts_with('.')) && !can.contains(' ') {
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
}
