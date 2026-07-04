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

// ===========================================================================
// Clipboard read bindings (winapi)
// ===========================================================================
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

    // IsClipboardFormatAvailable reports synthesizable formats too (e.g. CF_BITMAP
    // when only CF_DIB is present), so a single query answers "is X pasteable".
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

// ===========================================================================
// Image format resolution (port of Get-ImageFormat)
// ===========================================================================
mod imgutil {
    use std::path::Path;

    pub fn ext_of(path: &str) -> Option<String> {
        Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|s| !s.is_empty())
    }

    pub fn is_image(path: &str) -> bool {
        matches!(
            ext_of(path).as_deref(),
            Some("png") | Some("jpg") | Some("jpeg") | Some("bmp") | Some("gif")
                | Some("tif") | Some("tiff")
        )
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

    // Returns (System.Drawing.Imaging.ImageFormat name, mime, ext). --fmt wins, then
    // path extension, else PNG default — identical precedence to the PowerShell.
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

// ===========================================================================
// LLM fenced-block parser (port of ConvertFrom-LlmBundle)
// ===========================================================================
mod bundle {
    pub struct Rec {
        pub name: String,
        pub content: String,
    }

    // Content rejoined with CRLF, matching PS Environment.NewLine. Deviation from PS:
    // written WITHOUT a UTF-8 BOM (PS 5.1 [Text.Encoding]::UTF8 emits one) — see flags.
    pub fn parse(text: &str, fence: &str) -> Vec<Rec> {
        let lines: Vec<&str> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        let mut items = Vec::new();
        let mut in_block = false;
        let mut start = 0usize;
        let mut name = String::new();
        let mut i = 0usize;
        while i < lines.len() {
            let trimmed = lines[i].trim();
            if !in_block && trimmed.starts_with(fence) {
                in_block = true;
                name = if i > 0 {
                    lines[i - 1].trim().to_string()
                } else {
                    String::new()
                };
                start = i + 1;
                i += 1;
                continue;
            }
            if in_block && trimmed == fence {
                let content = lines[start..i].join("\r\n");
                items.push(Rec {
                    name: name.clone(),
                    content,
                });
                in_block = false;
            }
            i += 1;
        }
        if in_block {
            eprintln!("Warning: bundle ended inside a fenced block; content may be truncated.");
        }
        items
    }
}

// ===========================================================================
// Flags
// ===========================================================================
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
}

fn parse() -> Cfg {
    let mut c = Cfg {
        fence: "```".into(),
        ..Default::default()
    };
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--i" | "--image" => c.force_image = true,
            "--b64" | "--b" => c.as_b64 = true,
            "--data" | "--d" => c.as_data = true,
            "--files" | "--file" | "--f" => {} // auto-detected; accepted as no-op (PS parity)
            "--llm" | "--l" => c.from_llm = true,
            "--t" | "--trace" => c.trace = true,
            "--h" | "--help" => c.help = true,
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

// ===========================================================================
// Helpers
// ===========================================================================
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

// Deviation: no UTF-8 BOM (PS Set-Content/WriteAllText in 5.1 prepend one). See flags.
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

// Deviation: PS Write-Trace summary always ran (its guard was commented out); here it is
// gated behind --trace, honoring the traceEnabled flag the PS clearly intended.
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

const HELP: &str = "clipout — paste clipboard contents to disk

  USAGE
    clipout [destination] [flags]

  FLAGS
    --h --help        Show this message
    --t --trace       Verbose diagnostics + written-file summary
    --i --image       Treat clipboard as an image
    --b64             Write clipboard image as a Base64 text file
    --data            Write clipboard image as an HTML Base64 data URI file
    --files           Force file-drop handling (normally auto-detected)
    --llm             Extract an LLM fenced-block bundle to disk
    --fmt:<ext>       Output image format (png | jpg | bmp | gif | tif)
    --fence:<chars>   Fence marker for --llm (default: ```)

  MODES (auto-selected)
    clipout shot.png            Save a clipboard image
    clipout shot.jpg --fmt:jpg  Save, converting format
    clipout --llm               Extract every file from a bundle to cwd
    clipout ./dest/             Paste copied files into a directory
    clipout notes.txt           Write clipboard text to a file
    clipout                     Echo clipboard text to the console
";

// ===========================================================================
// Main
// ===========================================================================
fn main() {
    let cfg = parse();

    if cfg.help {
        print!("{}", HELP);
        return;
    }

    if cfg.trace {
        eprintln!(
            "  [PARSE] llm={} b64={} data={} forceImage={} fmt={:?} fence={:?} pos={:?}",
            cfg.from_llm, cfg.as_b64, cfg.as_data, cfg.force_image, cfg.fmt, cfg.fence, cfg.positional
        );
    }

    // Resolve destination: default cwd; relative -> absolute against cwd.
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

    // -----------------------------------------------------------------------
    // MODE: LLM bundle (checked first, mirrors PS ordering)
    // -----------------------------------------------------------------------
    if cfg.from_llm {
        let text = clipboard::get_text().unwrap_or_default();
        if text.trim().is_empty() {
            eprintln!("Clipboard is empty.");
            process::exit(1);
        }
        let base_dir: PathBuf = if dest_path.is_dir() {
            dest_path.clone()
        } else {
            match dest_path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => cwd(),
            }
        };
        let items = bundle::parse(&text, &cfg.fence);
        if items.is_empty() {
            eprintln!("No fenced blocks found in clipboard text.");
            process::exit(1);
        }
        for it in &items {
            let out = base_dir.join(&it.name);
            match write_text(&out, &it.content) {
                Ok(()) => {
                    println!("{}", it.name);
                    written.push(out);
                }
                Err(e) => eprintln!("Failed {}: {}", out.display(), e),
            }
        }
        println!("{} file(s) written from LLM bundle.", items.len());
        trace_summary(cfg.trace, &written);
        process::exit(0);
    }

    // -----------------------------------------------------------------------
    // MODE: Explorer file-drop (auto-detected)
    // -----------------------------------------------------------------------
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
        // Format override: --fmt: OR implied from destination extension (destination not a dir).
        let fmt_override: Option<String> = cfg.fmt.clone().or_else(|| {
            if !dest_path.is_dir() {
                imgutil::ext_of(&dest_path.to_string_lossy())
            } else {
                None
            }
        });

        for src in &dropped {
            let leaf = Path::new(src)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| src.clone());
            let base = Path::new(&leaf)
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| leaf.clone());
            let src_ext = imgutil::ext_of(src);
            let is_img = imgutil::is_image(src);

            let converts =
                is_img && fmt_override.is_some() && fmt_override.as_deref() != src_ext.as_deref();

            if converts {
                let target_ext = fmt_override.clone().unwrap();
                let (ps_fmt, _m, _e) =
                    imgutil::resolve_format(&format!("x.{}", target_ext), None);
                let out = dest_dir.join(format!("{}.{}", base, target_ext));
                ensure_parent(&out);
                match shim_transcode(src, &out.to_string_lossy(), ps_fmt) {
                    Ok(()) => {
                        println!("{}  (converted to {})", leaf, target_ext);
                        written.push(out);
                        continue;
                    }
                    Err(e) => eprintln!("Image conversion failed for '{}': {}", src, e),
                }
            }

            let out = dest_dir.join(&leaf);
            ensure_parent(&out);
            match fs::copy(src, &out) {
                Ok(_) => {
                    println!("{}", leaf);
                    written.push(out);
                }
                Err(e) => eprintln!("Copy failed for '{}': {}", src, e),
            }
        }
        println!("{} file(s) pasted.", dropped.len());
        trace_summary(cfg.trace, &written);
        process::exit(0);
    }

    // -----------------------------------------------------------------------
    // Image presence + directory-destination default text sink (PS skipWrite).
    // -----------------------------------------------------------------------
    let has_img = clipboard::has_format(clipboard::CF_BITMAP)
        || clipboard::has_format(clipboard::CF_DIB)
        || clipboard::has_format(clipboard::CF_DIBV5);

    let mut skip_write = false;
    if !has_img && dest_path.is_dir() {
        dest_path = dest_path.join("clipboard_output.txt");
        skip_write = true;
    }

    // -----------------------------------------------------------------------
    // MODE: Image (from clipboard). Trigger = explicit flag OR image-extension
    // destination. Fix vs PS: null-image ops guarded; dir-destination synthesises
    // a filename instead of crashing on Save(<directory>).
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // MODE: Plain text
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // No matching format. Tailored hint when an image is present but unaddressed.
    // -----------------------------------------------------------------------
    if has_img {
        eprintln!("Clipboard holds an image; name an output file (e.g. clipout shot.png) or pass --i.");
    } else {
        eprintln!("Clipboard does not contain text, an image, or files.");
    }
    process::exit(1);
}
