//! clipout — Rust port of clipout.ps1 (behavior-contract fidelity).
//! Sibling to clipin. Reads the Windows clipboard and writes it to disk.
//!
//! Placement:  src/bin/clipout.rs   (alongside src/main.rs = clipin)
//! Build:      cargo build --release      -> target/release/clipout.exe
//! Cargo.toml: winapi features must include winuser, winbase, shellapi, windef, minwindef
//!
//! Native paths : text write / echo, LLM-bundle extract, file-drop copy + transcode.
//! Shim paths   : clipboard-IMAGE extraction (raw save / base64 / data-uri) via a single
//!                STA PowerShell one-liner — mirrors clipin's raw-image shim, because
//!                CF_DIB -> encodable-bitmap reconstruction in pure winapi is ~200 LOC.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

mod clipboard {
    use std::os::windows::ffi::OsStringExt;
    use std::ptr;
    use winapi::shared::windef::HWND;
    use winapi::um::shellapi::{DragQueryFileW, HDROP};
    use winapi::um::winbase::{GlobalLock, GlobalUnlock};
    use winapi::um::winuser::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };

    pub const CF_UNICODETEXT: u32 = 13;
    pub const CF_HDROP: u32 = 15;
    pub const CF_BITMAP: u32 = 2;
    pub const CF_DIB: u32 = 8;
    pub const CF_DIBV5: u32 = 17;

    pub fn has_format(fmt: u32) -> bool {
        unsafe { IsClipboardFormatAvailable(fmt) != 0 }
    }

    fn open() -> bool {
        unsafe { OpenClipboard(ptr::null_mut::<HWND>() as HWND) != 0 }
    }

    pub fn get_text() -> Option<String> {
        if !open() {
            return None;
        }
        unsafe {
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                CloseClipboard();
                return None;
            }
            let p = GlobalLock(h) as *const u16;
            if p.is_null() {
                CloseClipboard();
                return None;
            }
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(p, len);
            let s = std::ffi::OsString::from_wide(slice)
                .into_string()
                .unwrap_or_default();
            GlobalUnlock(h);
            CloseClipboard();
            Some(s)
        }
    }

    pub fn get_file_drop() -> Option<Vec<String>> {
        if !open() {
            return None;
        }
        unsafe {
            let h = GetClipboardData(CF_HDROP);
            if h.is_null() {
                CloseClipboard();
                return None;
            }
            let hdrop = h as HDROP;
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, ptr::null_mut(), 0);
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let need = DragQueryFileW(hdrop, i, ptr::null_mut(), 0);
                let mut buf = vec![0u16; need as usize + 1];
                DragQueryFileW(hdrop, i, buf.as_mut_ptr(), need + 1);
                buf.truncate(need as usize);
                out.push(
                    std::ffi::OsString::from_wide(&buf)
                        .into_string()
                        .unwrap_or_default(),
                );
            }
            CloseClipboard();
            Some(out)
        }
    }
}

mod imgutil {
    use std::path::Path;

    pub fn ext_of(path: &str) -> Option<String> {
        Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|s| !s.is_empty())
    }

    pub fn is_image(path: &str) -> bool {
        ext_of(path)
            .as_deref()
            .map_or(false, |ext| matches!(ext, "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff"))
    }

    fn map(ext: &str) -> Option<(&'static str, &'static str)> {
        match ext {
            "png" => Some(("Png", "image/png")),
            "jpg" | "jpeg" => Some(("Jpeg", "image/jpeg")),
            "bmp" => Some(("Bmp", "image/bmp")),
            "gif" => Some(("Gif", "image/gif")),
            "tif" | "tiff" => Some(("Tiff", "image/tiff")),
            _ => None,
        }
    }

    pub fn resolve_format(path: &str, fmt_override: Option<&str>) -> (&'static str, String, String) {
        if let Some(o) = fmt_override {
            let o = o.to_lowercase();
            if let Some(t) = map(&o) {
                return (t.0, t.1.to_string(), o);
            }
        }
        if let Some(e) = ext_of(path) {
            if let Some(t) = map(&e) {
                return (t.0, t.1.to_string(), e);
            }
        }
        ("Png", "image/png".to_string(), "png".to_string())
    }
}

mod bundle {
    pub struct Rec {
        pub name: String,
        pub content: String,
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

        
        // ENTER BLOCK
        if !in_block && trimmed.starts_with(fence) {
            in_block = true;
            open_len = trimmed.chars().take_while(|&c| c == fence_char).count();

            // Filename is the single line immediately preceding the fence.
            let raw = if i > 0 { lines[i - 1].trim().to_string() } else { String::new() };

            let mut n = raw;
            while n.starts_with('#') {
                n = n.trim_start_matches('#').trim().to_string();
            }
            while n.starts_with('/') {
                n = n.trim_start_matches('/').trim().to_string();
            }
            n = n.replace('`', "").trim_end_matches(':').trim().to_string();

            // If no usable filename before the fence, peek at the first line inside the block.
            if !n.contains('.') && !n.contains(' ') && !n.contains(':') {
                if i + 1 < lines.len() {
                    let mut can = lines[i + 1].trim().to_string();
                    can = can.replace('`', "").trim_end_matches(':').trim().to_string();
                    while can.starts_with('/') {
                        can = can.trim_start_matches('/').trim().to_string();
                    }
                    if can.contains('.') && !can.contains(' ') && !can.contains(':'){
                        n = can;
                        start = i + 2; // skip the filename line itself
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

        // ------------------------------------------------------------
        // EXIT BLOCK — a pure run of the fence char, length >= the
        // opening run. A shorter run (e.g. a ``` example nested inside
        // a ```` wrapper) is content, not a close.
        // ------------------------------------------------------------
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

}

#[derive(Default)]
struct Cfg {
    force_image: bool,
    as_b64: bool,
    as_data: bool,
    from_llm: bool,
    trace: bool,
    help: bool,
    fmt: Option<String>,
    fence: String,
    positional: Option<String>,
    diff: bool,
    diff_target: Option<String>,
}

fn parse() -> Cfg {
    let mut c = Cfg {
        fence: "```".into(),
        ..Default::default()
    };

    let mut args = std::env::args().skip(1).peekable();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "/diff" | "--diff" => {
                c.diff = true;
                if let Some(target) = args.peek() {
                    c.diff_target = Some(target.clone());
                    args.next();
                }
            }
            "/image" | "/i" | "--i" | "-i" | "--image" => c.force_image = true,
            "/b64" | "/b" | "--b64" | "-b" | "--b" => c.as_b64 = true,
            "/data" | "/d" | "--data" | "-d" | "--d" => c.as_data = true,
            "/files" | "/file" | "/f" | "--files" | "--file" | "-f" | "--f" => {}
            "/llm" | "/l" | "--llm" | "--l" | "-llm" | "-l" => c.from_llm = true,
            "/trace" | "/t" | "--t" | "--trace" | "-t" => c.trace = true,
            "/help" | "/h" | "/?" | "--h" | "--help" | "-h" | "-?" | "?" => c.help = true,
            s if s.starts_with("--fmt:") => c.fmt = Some(s[6..].to_lowercase()),
            s if s.starts_with("--fence:") => c.fence = s[8..].to_string(),
            s if s.starts_with("--") => {}
            s => {
                if c.positional.is_none() {
                    c.positional = Some(s.to_string());
                }
            }
        }
    }
    c
}

fn add_commas(n: i64) -> String {
    let neg = n < 0;
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    let result: String = out.chars().rev().collect();
    if neg { format!("-{result}") } else { result }
}

fn preview_llm_items(items: &[bundle::Rec]) {
    use std::io::{self, Write};

    const MAX_LINES: usize = 3;
    const MAX_CHARS: usize = 120;

    const COLOR_FILE: &str = "\x1b[1;36m";
    const COLOR_LINE: &str = "\x1b[1;33m";
    const COLOR_RESET: &str = "\x1b[0m";
    const COLOR_LENGTH: &str = "\x1b[2;37m";
    const COLOR_LENGTH_DESC: &str = "\x1b[2;36m";

    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "").unwrap();

    for it in items {
        writeln!(out, "{}{}{} {}{} {}bytes{} {}{} {}lines{}", COLOR_FILE, it.name, COLOR_RESET, COLOR_LENGTH, add_commas(it.content.len() as i64), COLOR_LENGTH_DESC, COLOR_RESET, COLOR_LENGTH, add_commas(it.content.lines().count() as i64), COLOR_LENGTH_DESC, COLOR_RESET).unwrap();

        let lines: Vec<&str> = it
            .content
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();

        for line in lines.iter().take(MAX_LINES) {
            let mut s = line.to_string();
            if s.len() > MAX_CHARS {
                s.truncate(MAX_CHARS);
                s.push_str("…");
            }
            writeln!(out, "  {}{}{}", COLOR_LINE, s, COLOR_RESET).unwrap();
        }

        writeln!(out).unwrap();
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

fn ensure_parent(path: &Path) {
    if let Some(p) = path.parent() {
        if !p.as_os_str().is_empty() && !p.is_dir() {
            let _ = fs::create_dir_all(p);
        }
    }
}

fn write_text(path: &Path, content: &str) -> std::io::Result<()> {
    ensure_parent(path);
    fs::write(path, content.as_bytes())
}

enum ImgMode {
    Raw,
    Base64,
    DataUri,
}

fn run_ps(cmd: &str) -> Result<(), String> {
    match Command::new("powershell")
        .args(["-STA", "-NoProfile", "-NonInteractive", "-Command", cmd])
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) if s.code() == Some(3) => Err("Clipboard does not contain an image.".into()),
        Ok(s) => Err(format!("PowerShell image step failed (exit {:?}).", s.code())),
        Err(e) => Err(format!("PowerShell unavailable: {}", e)),
    }
}

fn shim_clipboard_image(mode: ImgMode, dest: &str, ps_fmt: &str, mime: &str) -> Result<(), String> {
    let d = ps_escape(dest);
    let body = match mode {
        ImgMode::Raw => format!(
            "$i.Save('{}',[System.Drawing.Imaging.ImageFormat]::{})",
            d, ps_fmt
        ),
        ImgMode::Base64 => format!(
            "$m=New-Object System.IO.MemoryStream;\
             $i.Save($m,[System.Drawing.Imaging.ImageFormat]::{});\
             [System.IO.File]::WriteAllText('{}',[Convert]::ToBase64String($m.ToArray()))",
            ps_fmt, d
        ),
        ImgMode::DataUri => format!(
            "$m=New-Object System.IO.MemoryStream;\
             $i.Save($m,[System.Drawing.Imaging.ImageFormat]::{});\
             [System.IO.File]::WriteAllText('{}','data:{};base64,'+[Convert]::ToBase64String($m.ToArray()))",
            ps_fmt, d, mime
        ),
    };
    let cmd = format!(
        "$ErrorActionPreference='Stop';\
         Add-Type -AssemblyName System.Drawing,System.Windows.Forms;\
         $i=[System.Windows.Forms.Clipboard]::GetImage();\
         if($null -eq $i){{exit 3}};{}",
        body
    );
    run_ps(&cmd)
}

fn shim_transcode(src: &str, dest: &str, ps_fmt: &str) -> Result<(), String> {
    let cmd = format!(
        "$ErrorActionPreference='Stop';\
         Add-Type -AssemblyName System.Drawing;\
         $i=[System.Drawing.Image]::FromFile('{}');\
         $i.Save('{}',[System.Drawing.Imaging.ImageFormat]::{});\
         $i.Dispose()",
        ps_escape(src),
        ps_escape(dest),
        ps_fmt
    );
    run_ps(&cmd)
}

fn trace_summary(on: bool, files: &[PathBuf]) {
    if !on || files.is_empty() {
        return;
    }
    eprintln!("  [SUMMARY] {} file(s):", files.len());
    for f in files {
        eprintln!("    {}", f.display());
        if let Ok(s) = fs::read_to_string(f) {
            let mut preview: String = s.lines().take(10).collect::<Vec<_>>().join("\n");
            if preview.chars().count() > 600 {
                preview = preview.chars().take(600).collect();
            }
            for line in preview.lines() {
                eprintln!("      {}", line);
            }
        }
    }
}

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

    // a_pos[i]/b_pos[i] = 0-indexed file position immediately BEFORE edits[i]
    // runs. Needed because an inserted-only or deleted-only hunk has no
    // Equal/Delete (resp. Insert) line inside it to read a start position
    // from — the position must come from the running cursor, not the body.
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

// =======================================================================
// COLORIZED RENDERING — replaces the old plain unified_diff.
//
// Verified by compiled execution (rustc 1.75) against 4 cases: token-swap
// modify, pure-append, pure-shrink, unpaired insert-only lines. See F
// below for the one confirmed (not hypothesized) limitation.
// =======================================================================

mod pal {
    pub const HEADER_FILE: &str = "\x1b[1;38;2;220;220;235m";
    pub const HEADER_HUNK_FG: &str = "\x1b[38;2;180;220;255m";
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

/// Char-level prefix/suffix trim to find the changed span within a paired
/// -/+ line. old_covered/new_covered are checked independently so a pure
/// append or pure shrink still emphasizes the side that actually changed
/// even when the other side is fully covered by prefix+suffix.
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

fn colorize_diff(name: &str, a: &[&str], b: &[&str], disk_content: &str, clip: &str, hunks: &[Hunk], max_changes: usize) {
    use pal::*;

    let a_no_trailing_nl = !disk_content.is_empty() && !disk_content.ends_with('\n');
    let b_no_trailing_nl = !clip.is_empty() && !clip.ends_with('\n');

    println!("{}--- a/{}{}", HEADER_FILE, name, RESET);
    println!("{}+++ b/{}{}", HEADER_FILE, name, RESET);

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

        println!(
            "{}{}@@ {}{} {}@@{}{}",
            HUNK_BG, HEADER_HUNK_FG, HEADER_HUNK_RANGE, range_str, HEADER_HUNK_FG, RESET, RESET
        );

        // Positional -/+ pairing within each contiguous change-run in the
        // hunk body, used only to decide which lines get intraline
        // emphasis. F: this is a heuristic, not real alignment — when a
        // change-run mixes a genuine modify with an unrelated pure
        // insert/delete, positional order can pair the wrong lines
        // together (confirmed via harness case1: a `return x;` -> `return
        // x + y;` modify sitting next to an unrelated `let y = 2;`
        // insertion got cross-paired). Cosmetic only: doesn't affect
        // which lines are marked -/+, only which spans get bold emphasis
        // within an already-correct line.
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

fn unified_diff(name: &str, clip: &str, disk: &Path, max_changes: usize) {
    let disk_content = fs::read_to_string(disk).unwrap_or_default();

    let a: Vec<&str> = disk_content.lines().collect();
    let b: Vec<&str> = clip.lines().collect();

    let edits = diff_lines(&a, &b);
    let hunks = build_hunks(&edits, 3);

    colorize_diff(name, &a, &b, &disk_content, clip, &hunks, max_changes);
}


#[cfg(test)]
mod tests {
    use super::*;

    fn reconstruct(a: &[&str], b: &[&str], edits: &[Edit]) -> (Vec<String>, Vec<String>) {
        let mut ra = Vec::new();
        let mut rb = Vec::new();
        for e in edits {
            match *e {
                Edit::Equal(ai, bi) => {
                    ra.push(a[ai].to_string());
                    rb.push(b[bi].to_string());
                }
                Edit::Delete(ai) => ra.push(a[ai].to_string()),
                Edit::Insert(bi) => rb.push(b[bi].to_string()),
            }
        }
        (ra, rb)
    }

    fn check_roundtrip(a_text: &str, b_text: &str) {
        let a: Vec<&str> = a_text.lines().collect();
        let b: Vec<&str> = b_text.lines().collect();
        let edits = diff_lines(&a, &b);
        let (ra, rb) = reconstruct(&a, &b, &edits);
        assert_eq!(ra, a, "reconstructed 'a' mismatch");
        assert_eq!(rb, b, "reconstructed 'b' mismatch");
    }

    #[test]
    fn roundtrip_basic_cases() {
        check_roundtrip("a\nb\nc\n", "a\nx\nc\n");
        check_roundtrip("", "a\nb\n");
        check_roundtrip("a\nb\n", "");
        check_roundtrip("same\nsame\nsame\n", "same\nsame\nsame\n");
        check_roundtrip(
            "one\ntwo\nthree\nfour\nfive\n",
            "one\nTWO\nthree\nfour\nFIVE\nsix\n",
        );
    }

    #[test]
    fn roundtrip_randomized() {
        let mut seed: u64 = 88172645463325252;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let na = (next() % 12) as usize;
            let a_lines: Vec<String> = (0..na).map(|_| format!("l{}", next() % 6)).collect();
            let mut b_lines = a_lines.clone();
            let mutations = (next() % 5) as usize;
            for _ in 0..mutations {
                let op = next() % 3;
                let pos = if b_lines.is_empty() {
                    0
                } else {
                    (next() as usize) % (b_lines.len() + 1)
                };
                match op {
                    0 if !b_lines.is_empty() && pos < b_lines.len() => {
                        b_lines.remove(pos);
                    }
                    1 => b_lines.insert(pos.min(b_lines.len()), format!("n{}", next() % 6)),
                    _ if !b_lines.is_empty() => {
                        let p = (next() as usize) % b_lines.len();
                        b_lines[p] = format!("m{}", next() % 6);
                    }
                    _ => {}
                }
            }
            let a_text = a_lines.join("\n") + if a_lines.is_empty() { "" } else { "\n" };
            let b_text = b_lines.join("\n") + if b_lines.is_empty() { "" } else { "\n" };
            check_roundtrip(&a_text, &b_text);
        }
    }
}

#[cfg(test)]
mod debug_test {
    use super::*;
    #[test]
    fn debug_case7() {
        let a_text = std::fs::read_to_string("/tmp/a7.txt").unwrap();
        let b_text = std::fs::read_to_string("/tmp/b7.txt").unwrap();
        let a: Vec<&str> = a_text.lines().collect();
        let b: Vec<&str> = b_text.lines().collect();
        let trace = myers_trace(&a, &b);
        let d = trace.len() - 1;
        let edits = backtrack(&a, &b, &trace);
        let changes = edits.iter().filter(|e| is_change(e)).count();
        println!("D from trace = {}, edits changes = {}", d, changes);
        assert_eq!(d, changes, "edit script length should equal D");
    }
}

#[cfg(test)]
mod debug_test2 {
    use super::*;
    #[test]
    fn debug_minimal_repro() {
        let a = vec!["line_5","line_6","line_1","line_0","line_1","line_4","line_3"];
        let b = vec!["line_5","line_6","new_1","line_1","chg_5","line_1","line_4","line_3"];
        let trace = myers_trace(&a, &b);
        println!("D = {}", trace.len() - 1);
        let edits = backtrack(&a, &b, &trace);
        for e in &edits { println!("{:?}", e); }
    }
}

#[cfg(test)]
mod debug_test3 {
    use super::*;
    #[test]
    fn debug_trace_dump() {
        let a = vec!["line_5","line_6","line_1","line_0","line_1","line_4","line_3"];
        let b = vec!["line_5","line_6","new_1","line_1","chg_5","line_1","line_4","line_3"];
        let n = a.len() as isize;
        let m = b.len() as isize;
        let max_d = n + m;
        let offset = max_d.max(1) as usize;
        let trace = myers_trace(&a, &b);
        for (d, v) in trace.iter().enumerate() {
            let d = d as isize;
            print!("d={}: ", d);
            let mut k = -d;
            while k <= d {
                let idx = (offset as isize + k) as usize;
                print!("k={} x={} | ", k, v[idx]);
                k += 2;
            }
            println!();
        }
    }
}

const HELP: &str = "\x1b[1;36mclipout\x1b[0m — paste clipboard contents to disk

  \x1b[0;34mUSAGE\x1b[0m
    clipout [destination] [flags]

  \x1b[0;34mFLAGS\x1b[0m
    \x1b[0;33m--h --help\x1b[0m        Show this message
    \x1b[0;33m--t --trace\x1b[0m       Verbose diagnostics + written-file summary
    \x1b[0;33m--i --image\x1b[0m       Treat clipboard as an image
    \x1b[0;33m--b64\x1b[0m             Write clipboard image as a Base64 text file
    \x1b[0;33m--data\x1b[0m            Write clipboard image as an HTML Base64 data URI file
    \x1b[0;33m--files\x1b[0m           Force file-drop handling (normally auto-detected)
    \x1b[0;33m--llm\x1b[0m             Extract an LLM fenced-block bundle to disk
    \x1b[0;33m--fmt:<ext>\x1b[0m       Output image format (png | jpg | bmp | gif | tif)
    \x1b[0;33m--fence:<chars>\x1b[0m   Fence marker for --llm (default: ```)

  \x1b[0;34mMODES\x1b[0m (auto-selected)
    \x1b[2;37mclipout shot.png\x1b[0m
    \x1b[2;37mclipout shot.jpg --fmt:jpg\x1b[0m
    \x1b[2;37mclipout --llm\x1b[0m
    \x1b[2;37mclipout ./dest/\x1b[0m
    \x1b[2;37mclipout notes.txt\x1b[0m
    \x1b[2;37mclipout\x1b[0m
";

fn main() {
    const COLOR_ERROR: &str = "\x1b[1;31m";
    const COLOR_WARNING: &str = "\x1b[0;33m";
    const COLOR_RESET: &str = "\x1b[0m";
    const COLOR_INFO: &str = "\x1b[0;34m";
    const COLOR_TRACE: &str = "\x1b[2;3m";
    const COLOR_DESC: &str = "\x1b[2;36m";
    let cfg = parse();

    if cfg.help {
        print!("{}", HELP);
        return;
    }

    if cfg.trace {
        eprintln!(
            "  {}[PARSE]{} {}llm{}={}{}{} {}b64{}={}{}{} {}data{}={}{}{} {}forceImage{}={}{}{} {}fmt{}={}{:?}{} {}fence{}={}{:?}{} {}pos{}={}{:?}{}",
            COLOR_TRACE, COLOR_RESET,
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.from_llm, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.as_b64, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.as_data, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.force_image, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.fmt, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.fence, COLOR_RESET, 
            COLOR_INFO, COLOR_RESET, COLOR_TRACE, cfg.positional, COLOR_RESET, 
        );
    }

    // Diff mode
    if cfg.diff {
        
        let text = clipboard::get_text().unwrap_or_default();
        let items = bundle::parse_fence(&text, &cfg.fence);

        if items.is_empty() {
            eprintln!("Clipboard does not contain text to diff.");
            process::exit(1);
        }

        for it in &items {
            unified_diff(&it.name, &it.content, Path::new(&it.name), 100);
        }
        process::exit(0);
    }

    {
        let text = clipboard::get_text().unwrap_or_default();
        if !text.trim().is_empty() {
            let items = bundle::parse_fence(&text, &cfg.fence);

            if cfg.positional.is_none() && !items.is_empty() && !cfg.from_llm {
                preview_llm_items(&items);
                process::exit(0);
            }

            if cfg.from_llm {
                let dest_path: PathBuf = match &cfg.positional {
                    Some(p) => {
                        let pb = PathBuf::from(p);
                        if pb.is_absolute() {
                            pb
                        } else {
                            cwd().join(pb)
                        }
                    }
                    None => cwd(),
                };

                let base_dir: PathBuf = if dest_path.is_dir() {
                    dest_path.clone()
                } else {
                    match dest_path.parent() {
                        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                        _ => cwd(),
                    }
                };

                let mut written: Vec<PathBuf> = Vec::new();

                for it in &items {
                    let name = it.name.replace('\\', "/");

                    if name.starts_with("../") || name.contains("/../") || name.contains(":") {
                        eprintln!("{}Skipping unsafe path{}: {}{}{}", 
                            COLOR_WARNING,
                            COLOR_RESET,
                            COLOR_INFO, name, COLOR_RESET);
                        continue;
                    }

                    let out = if Path::new(&name).is_absolute() {
                        PathBuf::from(&name)
                    } else {
                        base_dir.join(&name)
                    };

                    ensure_parent(&out);

                    match write_text(&out, &it.content) {
                        Ok(()) => {
                            println!("{}", out.display());
                            written.push(out);
                        }
                        Err(e) => eprintln!("{}Failed {}{}{}: {}{}{}", 
                            COLOR_ERROR, 
                            COLOR_INFO, out.display(), COLOR_RESET,
                            COLOR_INFO, e, COLOR_RESET),
                    }
                }

                println!("{}{} file(s) written{} {}LLM bundle{}.",
                    COLOR_DESC,
                    items.len(),
                    COLOR_RESET,
                    COLOR_INFO,
                    COLOR_RESET);
                trace_summary(cfg.trace, &written);
                process::exit(0);
            }
        }
    }

    let mut dest_path: PathBuf = match &cfg.positional {
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                cwd().join(pb)
            }
        }
        None => cwd(),
    };

    let mut written: Vec<PathBuf> = Vec::new();

    if clipboard::has_format(clipboard::CF_HDROP) {
        let dropped = clipboard::get_file_drop().unwrap_or_default();
        let dest_dir: PathBuf = if dest_path.is_dir() {
            dest_path.clone()
        } else {
            match dest_path.parent() {
                Some(p) if p.is_dir() => p.to_path_buf(),
                _ => cwd(),
            }
        };

        let fmt_override: Option<String> = cfg.fmt.clone().or_else(|| {
            if !dest_path.is_dir() {
                imgutil::ext_of(&dest_path.to_string_lossy())
            } else {
                None
            }
        });

        for src in &dropped {
            let src_path = Path::new(src);
            let base = Path::new(&dropped[0])
                .parent()
                .unwrap_or_else(|| Path::new(""));
            let rel = src_path.strip_prefix(base).unwrap_or(src_path);

            let src_ext = imgutil::ext_of(src);
            let is_img = imgutil::is_image(src);

            let converts =
                is_img && fmt_override.is_some() && fmt_override.as_deref() != src_ext.as_deref();

            if converts {
                let target_ext = fmt_override.clone().unwrap();
                let (ps_fmt, _m, _e) =
                    imgutil::resolve_format(&format!("x.{}", target_ext), None);

                let mut out2 = dest_dir.join(rel);
                out2.set_extension(&target_ext);

                ensure_parent(&out2);

                match shim_transcode(src, &out2.to_string_lossy(), ps_fmt) {
                    Ok(()) => {
                        println!("{}  ({}converted to {}{})", rel.display(), COLOR_DESC, target_ext, COLOR_RESET);
                        written.push(out2);
                        continue;
                    }
                    Err(e) => eprintln!("{}Image conversion failed for '{}': {}{}", COLOR_ERROR, src, e, COLOR_RESET),
                }
            }

            let out = dest_dir.join(rel);
            ensure_parent(&out);

            match fs::copy(src, &out) {
                Ok(_) => {
                    println!("{}", rel.display());
                    written.push(out);
                }
                Err(e) => eprintln!("{}Copy failed for '{}': {}{}", COLOR_ERROR, src, e, COLOR_RESET),
            }
        }

        println!("{}{} file(s) pasted.{}", COLOR_DESC, dropped.len(), COLOR_RESET);
        trace_summary(cfg.trace, &written);
        process::exit(0);
    }

    let has_img = clipboard::has_format(clipboard::CF_BITMAP)
        || clipboard::has_format(clipboard::CF_DIB)
        || clipboard::has_format(clipboard::CF_DIBV5);

    let mut skip_write = false;
    if !has_img && dest_path.is_dir() {
        dest_path = dest_path.join("clipboard_output.txt");
        skip_write = true;
    }

    let explicit_image = cfg.force_image || cfg.as_b64 || cfg.as_data;
    let auto_image = imgutil::is_image(&dest_path.to_string_lossy());
    let want_image = explicit_image || auto_image;

    if want_image && has_img {
        if dest_path.is_dir() {
            let ext = cfg.fmt.clone().unwrap_or_else(|| "png".into());
            dest_path = dest_path.join(format!("clipboard.{}", ext));
        }

        let (ps_fmt, mime, ext) =
            imgutil::resolve_format(&dest_path.to_string_lossy(), cfg.fmt.as_deref());

        ensure_parent(&dest_path);
        let dest_s = dest_path.to_string_lossy().to_string();

        let (mode, tail) = if cfg.as_b64 {
            (
                ImgMode::Base64,
                format!("Image written as Base64 ({}) {}", ext.to_uppercase(), dest_s),
            )
        } else if cfg.as_data {
            (
                ImgMode::DataUri,
                format!("Image written as HTML Base64 data URI ({}) {}", mime, dest_s),
            )
        } else {
            (
                ImgMode::Raw,
                format!("Image written ({}) {}", ext.to_uppercase(), dest_s),
            )
        };

        match shim_clipboard_image(mode, &dest_s, ps_fmt, &mime) {
            Ok(()) => {
                println!("{}", tail);
                written.push(dest_path.clone());
                trace_summary(cfg.trace, &written);
                process::exit(0);
            }
            Err(e) => {
                eprintln!("{}", e);
                process::exit(1);
            }
        }
    } else if want_image && !has_img {
        eprintln!("Clipboard does not contain an image.");
        process::exit(1);
    }

    if clipboard::has_format(clipboard::CF_UNICODETEXT) {
        let text = clipboard::get_text().unwrap_or_default();

        if !skip_write {
            match write_text(&dest_path, &text) {
                Ok(()) => {
                    println!("Text written {}", dest_path.display());
                    written.push(dest_path.clone());
                }
                Err(e) => {
                    eprintln!("Write failed: {}", e);
                    process::exit(1);
                }
            }
        } else {
            println!();
            println!("{}", text);
            println!();
        }

        trace_summary(cfg.trace, &written);
        process::exit(0);
    }

    if has_img {
        eprintln!("Clipboard holds an image; name an output file (e.g. clipout shot.png) or pass --i.");
    } else {
        eprintln!("Clipboard does not contain text, an image, or files.");
    }

    process::exit(1);
}