use crate::config::Cfg;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
enum Edit {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

fn is_change(e: &Edit) -> bool {
    !matches!(e, Edit::Equal(_, _))
}

fn myers_trace(a: &[&str], b: &[&str]) -> Vec<Vec<isize>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max_d = n + m;
    let width = (2 * max_d.max(1) + 1) as usize;
    let offset = max_d.max(1) as usize;

    let mut v = vec![0isize; width];
    let mut trace = Vec::with_capacity((max_d + 1) as usize);

    if max_d == 0 {
        trace.push(v);
        return trace;
    }

    for d in 0..=max_d {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (offset as isize + k) as usize;
            let down = k == -d || (k != d && v[idx - 1] <= v[idx + 1]);
            let mut x = if down { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = x - k;

            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;

            if x >= n && y >= m {
                return trace;
            }
            k += 2;
        }
    }
    trace
}

fn backtrack(a: &[&str], b: &[&str], trace: &[Vec<isize>]) -> Vec<Edit> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max_d = (n + m).max(1);
    let offset = max_d as usize;

    let mut x = n;
    let mut y = m;
    let mut edits = Vec::new();

    for d in (0..trace.len()).rev() {
        let v = &trace[d];
        let d = d as isize;
        let k = x - y;
        let idx = |k: isize| (offset as isize + k) as usize;

        let down = k == -d || (k != d && v[idx(k - 1)] <= v[idx(k + 1)]);
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = v[idx(prev_k)];
        let prev_y = prev_x - prev_k;

        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            edits.push(Edit::Equal(x as usize, y as usize));
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                edits.push(Edit::Insert(y as usize));
            } else {
                x -= 1;
                edits.push(Edit::Delete(x as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }

    edits.reverse();
    edits
}

fn diff_lines(a: &[&str], b: &[&str]) -> Vec<Edit> {
    let trace = myers_trace(a, b);
    backtrack(a, b, &trace)
}

struct Hunk {
    a_start: usize,
    a_count: usize,
    b_start: usize,
    b_count: usize,
    body: Vec<(char, usize)>,
}

fn build_hunks(edits: &[Edit], context: usize) -> Vec<Hunk> {
    let n = edits.len();

    let mut a_pos = vec![0usize; n + 1];
    let mut b_pos = vec![0usize; n + 1];
    for (i, e) in edits.iter().enumerate() {
        let (da, db) = match e {
            Edit::Equal(_, _) => (1, 1),
            Edit::Delete(_) => (1, 0),
            Edit::Insert(_) => (0, 1),
        };
        a_pos[i + 1] = a_pos[i] + da;
        b_pos[i + 1] = b_pos[i] + db;
    }

    let mut hunks = Vec::new();
    let mut i = 0;

    while i < n {
        if !is_change(&edits[i]) {
            i += 1;
            continue;
        }

        let mut start = i;
        let mut back = 0;
        while start > 0 && back < context && !is_change(&edits[start - 1]) {
            start -= 1;
            back += 1;
        }

        let mut end = i;
        loop {
            while end < n && is_change(&edits[end]) {
                end += 1;
            }
            let mut probe = end;
            let mut equal_run = 0;
            while probe < n && !is_change(&edits[probe]) && equal_run <= 2 * context {
                probe += 1;
                equal_run += 1;
            }
            if probe < n && is_change(&edits[probe]) && equal_run <= 2 * context {
                end = probe;
            } else {
                break;
            }
        }

        let trail = context.min(n - end);
        let hunk_end = end + trail;

        let mut body = Vec::with_capacity(hunk_end - start);
        let (mut a_count, mut b_count) = (0usize, 0usize);

        for e in &edits[start..hunk_end] {
            match *e {
                Edit::Equal(ai, _) => {
                    a_count += 1;
                    b_count += 1;
                    body.push((' ', ai));
                }
                Edit::Delete(ai) => {
                    a_count += 1;
                    body.push(('-', ai));
                }
                Edit::Insert(bi) => {
                    b_count += 1;
                    body.push(('+', bi));
                }
            }
        }

        hunks.push(Hunk {
            a_start: a_pos[start],
            a_count,
            b_start: b_pos[start],
            b_count,
            body,
        });

        i = hunk_end;
    }

    hunks
}

mod pal {
    pub const HEADER_FILE: &str = "\x1b[1;38;2;220;220;235m";
    pub const UNNAMED_FILE: &str = "\x1b[1;38;2;240;180;90m";
    pub const HEADER_HUNK_FG: &str = "\x1b[38;2;180;220;255m";
    pub const UNNAMED_HUNK_FG: &str = "\x1b[38;2;255;210;140m";
    pub const HEADER_HUNK_RANGE: &str = "\x1b[1;38;2;120;200;255m";
    pub const DEL_FG: &str = "\x1b[38;2;255;180;180m";
    pub const INS_FG: &str = "\x1b[38;2;170;255;190m";
    pub const CTX_FG: &str = "\x1b[38;2;150;150;160m";
    pub const NO_NEWLINE: &str = "\x1b[3;38;2;140;140;150m";

    pub const DEL_BG: &str = "\x1b[48;2;60;20;20m";
    pub const INS_BG: &str = "\x1b[48;2;20;55;25m";
    pub const HUNK_BG: &str = "\x1b[48;2;20;30;45m";

    pub const DEL_EMPH: &str = "\x1b[1;38;2;255;255;255;48;2;140;35;35m";
    pub const INS_EMPH: &str = "\x1b[1;38;2;255;255;255;48;2;35;140;60m";

    pub const RESET: &str = "\x1b[0m";
}

fn common_prefix_len(a: &[char], b: &[char]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn common_suffix_len(a: &[char], b: &[char], prefix: usize) -> usize {
    let a_rest = &a[prefix..];
    let b_rest = &b[prefix..];
    a_rest
        .iter()
        .rev()
        .zip(b_rest.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

fn render_intraline(old: &str, new: &str) -> (String, String) {
    let oc: Vec<char> = old.chars().collect();
    let nc: Vec<char> = new.chars().collect();

    let max_prefix = oc.len().min(nc.len());
    let prefix = common_prefix_len(&oc, &nc).min(max_prefix);
    let max_suffix = oc.len().min(nc.len()) - prefix;
    let suffix = common_suffix_len(&oc, &nc, prefix).min(max_suffix);

    let old_covered = prefix + suffix >= oc.len();
    let new_covered = prefix + suffix >= nc.len();

    if old_covered && new_covered {
        return (
            format!("{}{}{}", pal::DEL_FG, old, pal::RESET),
            format!("{}{}{}", pal::INS_FG, new, pal::RESET),
        );
    }

    let old_pre: String = oc[..prefix].iter().collect();
    let old_mid: String = if old_covered {
        String::new()
    } else {
        oc[prefix..oc.len() - suffix].iter().collect()
    };
    let old_suf: String = oc[oc.len() - suffix..].iter().collect();

    let new_pre: String = nc[..prefix].iter().collect();
    let new_mid: String = if new_covered {
        String::new()
    } else {
        nc[prefix..nc.len() - suffix].iter().collect()
    };
    let new_suf: String = nc[nc.len() - suffix..].iter().collect();

    let old_out = if old_mid.is_empty() {
        format!("{}{}{}{}", pal::DEL_FG, old_pre, old_suf, pal::RESET)
    } else {
        format!(
            "{}{}{}{}{}{}{}{}",
            pal::DEL_FG, old_pre, pal::DEL_EMPH, old_mid, pal::RESET, pal::DEL_BG, pal::DEL_FG, old_suf
        )
    };
    let new_out = if new_mid.is_empty() {
        format!("{}{}{}{}", pal::INS_FG, new_pre, new_suf, pal::RESET)
    } else {
        format!(
            "{}{}{}{}{}{}{}{}",
            pal::INS_FG, new_pre, pal::INS_EMPH, new_mid, pal::RESET, pal::INS_BG, pal::INS_FG, new_suf
        )
    };

    (old_out, new_out)
}

pub fn colorize_diff(name: &str, unnamed: bool, a: &[&str], b: &[&str], disk_content: &str, clip: &str, hunks: &[Hunk], max_changes: usize) {
    use pal::*;

    let a_no_trailing_nl = !disk_content.is_empty() && !disk_content.ends_with('\n');
    let b_no_trailing_nl = !clip.is_empty() && !clip.ends_with('\n');

    let file_color = if unnamed { UNNAMED_FILE } else { HEADER_FILE };

    println!("{}--- a/{}{}", file_color, name, RESET);
    println!("{}+++ b/{}{}", file_color, name, RESET);

    let mut changes_emitted = 0usize;

    for hunk in hunks {
        let fmt_range = |start: usize, count: usize| {
            if count == 0 {
                format!("{},0", start)
            } else if count == 1 {
                format!("{}", start + 1)
            } else {
                format!("{},{}", start + 1, count)
            }
        };

        let range_str = format!(
            "-{} +{}",
            fmt_range(hunk.a_start, hunk.a_count),
            fmt_range(hunk.b_start, hunk.b_count)
        );

        let hunk_header_fg = if unnamed { UNNAMED_HUNK_FG } else { HEADER_HUNK_FG };

        // Fixed format string argument count (7 arguments for 7 format specifiers)
        println!(
            "{}{}{}@@ {}{} {}@@{}",
            HUNK_BG, hunk_header_fg, HEADER_HUNK_RANGE, range_str, hunk_header_fg, RESET, RESET
        );

        let mut pair_of: Vec<Option<usize>> = vec![None; hunk.body.len()];
        {
            let mut idx = 0;
            while idx < hunk.body.len() {
                if hunk.body[idx].0 == '-' {
                    let del_start = idx;
                    let mut del_end = idx;
                    while del_end < hunk.body.len() && hunk.body[del_end].0 == '-' {
                        del_end += 1;
                    }
                    let ins_start = del_end;
                    let mut ins_end = ins_start;
                    while ins_end < hunk.body.len() && hunk.body[ins_end].0 == '+' {
                        ins_end += 1;
                    }
                    let pair_count = (del_end - del_start).min(ins_end - ins_start);
                    for k in 0..pair_count {
                        pair_of[del_start + k] = Some(ins_start + k);
                        pair_of[ins_start + k] = Some(del_start + k);
                    }
                    idx = ins_end.max(del_end);
                } else {
                    idx += 1;
                }
            }
        }

        for (i, &(marker, idx)) in hunk.body.iter().enumerate() {
            let (raw_line, is_last_of_side, no_nl) = match marker {
                '-' => (a[idx], idx + 1 == a.len(), a_no_trailing_nl),
                '+' => (b[idx], idx + 1 == b.len(), b_no_trailing_nl),
                _ => (a[idx], idx + 1 == a.len(), a_no_trailing_nl),
            };

            match marker {
                '-' => {
                    let rendered = if let Some(j) = pair_of[i] {
                        if hunk.body[j].0 == '+' {
                            let (old_out, _) = render_intraline(raw_line, b[hunk.body[j].1]);
                            old_out
                        } else {
                            format!("{}{}{}", DEL_FG, raw_line, RESET)
                        }
                    } else {
                        format!("{}{}{}", DEL_FG, raw_line, RESET)
                    };
                    println!("{}{}-{}{}", DEL_BG, DEL_FG, rendered, RESET);
                    changes_emitted += 1;
                }
                '+' => {
                    let rendered = if let Some(j) = pair_of[i] {
                        if hunk.body[j].0 == '-' {
                            let (_, new_out) = render_intraline(a[hunk.body[j].1], raw_line);
                            new_out
                        } else {
                            format!("{}{}{}", INS_FG, raw_line, RESET)
                        }
                    } else {
                        format!("{}{}{}", INS_FG, raw_line, RESET)
                    };
                    println!("{}{}+{}{}", INS_BG, INS_FG, rendered, RESET);
                    changes_emitted += 1;
                }
                _ => {
                    println!("{} {}{}", CTX_FG, raw_line, RESET);
                }
            }

            if is_last_of_side && no_nl {
                println!("{}\\ No newline at end of file{}", NO_NEWLINE, RESET);
            }

            if changes_emitted >= max_changes {
                println!(
                    "\n\x1b[1;33m⚡ diff truncated after {max_changes} changed lines ⚡{}",
                    RESET
                );
                return;
            }
        }
    }
}

pub fn unified_diff(_cfg: &Cfg, name: &str, unnamed: bool, clip: &str, disk: &Path, max_changes: usize) {
    let disk_content = fs::read_to_string(disk).unwrap_or_default();

    let a: Vec<&str> = disk_content.lines().collect();
    let b: Vec<&str> = clip.lines().collect();

    let edits = diff_lines(&a, &b);
    let hunks = build_hunks(&edits, 3);

    colorize_diff(name, unnamed, &a, &b, &disk_content, clip, &hunks, max_changes);
}