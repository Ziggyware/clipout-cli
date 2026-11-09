#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rec {
    pub name: String,
    pub content: String,
    pub unnamed: bool,
}

pub fn filter_items(items: Vec<Rec>, include_unnamed: bool) -> Vec<Rec> {
    if include_unnamed {
        return items;
    }
    items.into_iter().filter(|it| !it.unnamed).collect()
}

fn marker_trim(s: &str) -> &str {
    s.trim_matches(|c| {
        matches!(c, '*' | '_' | '(' | ')' | '-' | '+' | '~' | '[' | ']' | '`' | ':')
    })
    .trim()
}

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

fn is_valid_path_token(s: &str) -> bool {
    if s.is_empty() || s.len() > 160 || s.contains(char::is_whitespace) {
        return false;
    }
    if s.contains("://") || s.starts_with("--") {
        return false;
    }
    if s.chars().any(|c| {
        matches!(
            c,
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '`' | '\'' | ';' | ',' | '!' | '[' | ']' | '(' | ')' | '{' | '}'
        )
    }) {
        return false;
    }
    if s.ends_with('.') || s.ends_with('/') || s.ends_with('\\') {
        return false;
    }

    let has_sep = s.contains('/') || s.contains('\\');
    let file_part = s.rsplit(['/', '\\']).next().unwrap_or(s);

    if let Some(dot_pos) = file_part.rfind('.') {
        let ext = &file_part[dot_pos + 1..];
        let valid_ext = !ext.is_empty()
            && ext.len() <= 12
            && ext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'));
        if !valid_ext {
            return false;
        }
        let stem = &file_part[..dot_pos];
        if stem.is_empty() && !file_part.starts_with('.') {
            return false;
        }
        true
    } else {
        has_sep
            && file_part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    }
}

fn extract_filename_from_line(raw_line: &str) -> Option<String> {
    let mut cleaned = raw_line.trim();
    if cleaned.is_empty() {
        return None;
    }

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

    if let Some((lhs, rhs)) = cleaned.split_once(':') {
        let rhs_clean = marker_trim(strip_number_prefix(marker_trim(rhs)));
        if !rhs_clean.is_empty() && is_valid_path_token(rhs_clean) {
            return Some(rhs_clean.to_string());
        }
        let lhs_clean = marker_trim(strip_number_prefix(marker_trim(lhs)));
        if rhs.trim().is_empty() && is_valid_path_token(lhs_clean) {
            return Some(lhs_clean.to_string());
        }
    }

    let cleaned = marker_trim(cleaned);
    let cleaned = marker_trim(strip_number_prefix(cleaned));

    if !cleaned.contains(char::is_whitespace) {
        if is_valid_path_token(cleaned) {
            return Some(cleaned.to_string());
        }
        return None;
    }

    if let Some(space_idx) = cleaned.find(char::is_whitespace) {
        let first_word = marker_trim(&cleaned[..space_idx]);
        let rest = cleaned[space_idx..].trim_start();

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
            if non_empty_seen > 0 {
                break;
            }
            continue;
        }

        if raw.starts_with("```") || raw.starts_with("~~~") {
            break;
        }

        non_empty_seen += 1;
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
    let mut is_unnamed = false;
    let mut open_len = 0usize;
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if !in_block && trimmed.starts_with(fence) {
            in_block = true;
            open_len = trimmed.chars().take_while(|&c| c == fence_char).count();

            let info_str = trimmed[open_len..].trim();
            let info_filename = info_str
                .split_once(':')
                .map(|(_, rhs)| rhs.trim())
                .or_else(|| {
                    info_str
                        .split_whitespace()
                        .find(|t| t.contains('/') || t.contains('\\') || t.contains('.'))
                })
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
                        is_unnamed = false;
                        start = i + 2;
                    } else {
                        n = format!("file_{}.txt", items.len());
                        is_unnamed = true;
                        start = i + 1;
                    }
                } else {
                    n = format!("file_{}.txt", items.len());
                    is_unnamed = true;
                    start = i + 1;
                }
            } else {
                is_unnamed = false;
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
                    unnamed: is_unnamed,
                });
                in_block = false;
            }
        }

        i += 1;
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
        assert!(!items[0].unnamed);
    }

    #[test]
    fn filter_items_drops_unnamed_by_default_and_keeps_when_forced() {
        let text = "Cargo.toml\n```toml\n[package]\n```\n\nBuild and run:\n```powershell\ncargo build\n```\n";
        let raw = parse(text);
        assert_eq!(raw.len(), 2);
        assert!(!raw[0].unnamed);
        assert!(raw[1].unnamed);

        let default_filtered = filter_items(raw.clone(), false);
        assert_eq!(default_filtered.len(), 1);
        assert_eq!(default_filtered[0].name, "Cargo.toml");

        let forced = filter_items(raw, true);
        assert_eq!(forced.len(), 2);
        assert_eq!(forced[0].name, "Cargo.toml");
        assert_eq!(forced[1].name, "file_1.txt");
    }
}