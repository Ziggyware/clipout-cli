pub struct Rec {
    pub name: String,
    pub content: String,
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

        cleaned = cleaned.trim_matches(|c| {
            matches!(c, '*' | '(' | ')' | '-' | '+' | '~' | '[' | ']' | '`' | ':')
        }).trim();

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
