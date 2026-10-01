//! clipout — Rust port of clipout.ps1 (behavior-contract fidelity).
//! Sibling to clipin. Reads the Windows clipboard and writes it to disk.
//!
//! Placement:  src/bin/clipout.rs   (alongside src/main.rs = clipin)
//! Build:      cargo build --release -> target/release/clipout.exe
//! Cargo.toml: winapi features must include winuser, winbase, shellapi, windef, minwindef
//!
//! Native paths : text write / echo, LLM-bundle extract, file-drop copy + transcode.
//! Shim paths   : clipboard-IMAGE extraction (raw save / base64 / data-uri) via a single
//!                STA PowerShell one-liner — mirrors clipin's raw-image shim, because
//!                CF_DIB -> encodable-bitmap reconstruction in pure winapi is ~200 LOC.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

mod config;
mod clipboard;
mod imgutil;
mod bundle;
mod diff;

use config::Cfg;

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

pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

pub fn ensure_parent(path: &Path) {
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
         if($null -eq$i){{exit 3}};{}",
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

const HELP: &str = "\x1b[1;36mclipout\x1b[0m — paste clipboard contents to disk

  \x1b[0;34mUSAGE\x1b[0m
    clipout [destination] [flags]

  \x1b[0;34mFLAGS\x1b[0m
    \x1b[0;33m--h --help\x1b[0m        Show this message
    \x1b[0;33m--t --trace\x1b[0m       Verbose diagnostics + written-file summary
    \x1b[0;33m--i --image\x1b[0m       Treat clipboard as an image
    \x1b[0;33m--view\x1b[0m            View full clipboard text
    \x1b[0;33m--b64\x1b[0m             Write clipboard image as a Base64 text file
    \x1b[0;33m--data\x1b[0m            Write clipboard image as an HTML Base64 data URI file
    \x1b[0;33m--files\x1b[0m           Force file-drop handling (normally auto-detected)
    \x1b[0;33m--llm\x1b[0m             Extract an LLM fenced-block bundle to disk
    \x1b[0;33m--fmt:<ext>\x1b[0m       Output image format (png | jpg | bmp | gif | tif)
    \x1b[0;33m--fence:<chars>\x1b[0m   Fence marker for --llm (default: ```)
    \x1b[0;33m/diff [target]\x1b[0m    Diff clipboard against disk. Plain-text clipboard
                          requires [target]; a fenced bundle diffs each
                          file against its same-named counterpart under
                          [target] (or its parent dir), default cwd.

  \x1b[0;34mMODES\x1b[0m (auto-selected)
    \x1b[2;37mclipout shot.png\x1b[0m
    \x1b[2;37mclipout shot.jpg --fmt:jpg\x1b[0m
    \x1b[2;37mclipout --llm\x1b[0m
    \x1b[2;37mclipout ./dest/\x1b[0m
    \x1b[2;37mclipout notes.txt\x1b[0m
    \x1b[2;37mclipout /diff src/main.rs\x1b[0m
    \x1b[2;37mclipout /diff ./src/\x1b[0m
    \x1b[2;37mclipout\x1b[0m
";

fn main() {
    const COLOR_ERROR: &str = "\x1b[1;31m";
    const COLOR_WARNING: &str = "\x1b[0;33m";
    const COLOR_RESET: &str = "\x1b[0m";
    const COLOR_INFO: &str = "\x1b[0;34m";
    const COLOR_TRACE: &str = "\x1b[2;3m";
    const COLOR_DESC: &str = "\x1b[2;36m";
    let cfg = config::parse();

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

    if cfg.diff {
        let text = clipboard::get_text().unwrap_or_default();

        let target_is_file = match &cfg.diff_target {
            Some(p) => {
                let looks_like_dir = p.ends_with('/') || p.ends_with('\\');
                let pb = if Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    cwd().join(p)
                };
                !looks_like_dir && !pb.is_dir()
            }
            None => false,
        };

        if target_is_file {
            let p = cfg.diff_target.as_ref().unwrap();
            let target = if Path::new(p).is_absolute() {
                PathBuf::from(p)
            } else {
                cwd().join(p)
            };

            diff::unified_diff(&cfg,
                target
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
                &text,
                &target,
                100,
            );

            process::exit(0);
        }

        let items = bundle::parse_fence(&text, &cfg.fence);

        if items.is_empty() {
            let target = match &cfg.diff_target {
                Some(p) => PathBuf::from(p),
                None => {
                    eprintln!("clipout /diff requires a target file when clipboard is plain text.");
                    process::exit(1);
                }
            };

            diff::unified_diff(&cfg,
                target
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
                &text,
                &target,
                100,
            );

            process::exit(0);
        }

        let base_dir: PathBuf = match &cfg.diff_target {
            Some(p) => {
                let pb = if Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    cwd().join(p)
                };
                if pb.is_dir() {
                    pb
                } else {
                    match pb.parent() {
                        Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                        _ => cwd(),
                    }
                }
            }
            None => cwd(),
        };

        let mut diffed = 0usize;
        let mut skipped = 0usize;

        for (idx, it) in items.iter().enumerate() {
            let name = it.name.replace('\\', "/");

            if name.starts_with("../") || name.contains("/../") || name.contains(':') {
                eprintln!(
                    "{}Skipping unsafe path{}: {}{}{}",
                    COLOR_WARNING, COLOR_RESET, COLOR_INFO, name, COLOR_RESET
                );
                skipped += 1;
                continue;
            }

            let disk_path = if Path::new(&name).is_absolute() {
                PathBuf::from(&name)
            } else {
                base_dir.join(&name)
            };

            if idx > 0 {
                println!();
            }

            diff::unified_diff(&cfg, &name, &it.content, &disk_path, 100);
            diffed += 1;
        }

        eprintln!(
            "\n{}{} file(s) diffed{}, {}{} skipped{}.",
            COLOR_DESC, diffed, COLOR_RESET, COLOR_WARNING, skipped, COLOR_RESET
        );

        process::exit(if diffed > 0 { 0 } else { 1 });
    }

    if cfg.view {
        let text = clipboard::get_text().unwrap_or_default();
        println!("{}", text);
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
                        eprintln!(
                            "{}Skipping unsafe path{}: {}{}{}",
                            COLOR_WARNING, COLOR_RESET, COLOR_INFO, name, COLOR_RESET
                        );
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
                        Err(e) => eprintln!(
                            "{}Failed {}{}{}: {}{}{}",
                            COLOR_ERROR, COLOR_INFO, out.display(), COLOR_RESET, COLOR_INFO, e, COLOR_RESET
                        ),
                    }
                }

                println!(
                    "{}{} file(s) written{} {}LLM bundle{}.",
                    COLOR_DESC, items.len(), COLOR_RESET, COLOR_INFO, COLOR_RESET
                );
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
