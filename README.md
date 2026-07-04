# clipout

A fast, single-binary clipboard-**output** utility for Windows — paste whatever is on the clipboard to disk: images, copied files, LLM fenced-block bundles, or text. The inverse of [clipin](https://github.com/ziggyware/clipin-cli).

Rust port of the original `clipout.ps1`, trading ~300–500 ms of interpreter startup for a native executable that starts in single-digit milliseconds. The flag surface and mode contract reproduce the PowerShell original.

```
clipout [destination] [flags]
```

## Why

The PowerShell version paid a fixed interpreter tax on every paste and needed STA-thread re-entry for COM clipboard access. This port keeps the same behavior while removing both costs — no runtime, no re-launch, one small executable callable from any shell.

## Install

### Build from source

Requires the Rust toolchain ([rustup.rs](https://rustup.rs/)) with the default MSVC target.

```powershell
git clone https://github.com/ziggyware/clipout-cli.git
cd clipout-cli
cargo build --release
```

The binary lands at `target\release\clipout.exe`.

### Put it on your PATH

```powershell
Copy-Item .\target\release\clipout.exe C:\Windows\System32\ -Force
```

Then `clipout` works from any directory. To confirm which binary resolves:

```powershell
Get-Command clipout -All | Format-Table CommandType, Name, Source -Auto
```

## Usage

### Flags

| Flag | Effect |
|------|--------|
| `--h` `--help` | Show help |
| `--t` `--trace` | Verbose diagnostics plus a written-file summary |
| `--i` `--image` | Treat the clipboard as an image |
| `--b64` | Write the clipboard image as a Base64 text file |
| `--data` | Write the clipboard image as an HTML Base64 data URI file |
| `--files` | Force file-drop handling (normally auto-detected) |
| `--llm` | Extract an LLM fenced-block bundle to disk |
| `--fmt:<ext>` | Output image format (`png` \| `jpg` \| `bmp` \| `gif` \| `tif`) |
| `--fence:<chars>` | Fence marker for `--llm` (default: three backticks) |

### Modes

`clipout` selects a mode from the flags, the clipboard contents, and the destination:

- **Image** — a clipboard bitmap written to the given file. `--b64` and `--data` emit text encodings instead of a binary image; `--fmt:` transcodes on the way out.
- **File-drop** — files copied to the clipboard from Explorer are pasted into the destination directory. Auto-detected.
- **LLM bundle** (`--llm`) — parses clipboard text as fenced blocks and writes each embedded file to disk, recreating the filenames.
- **Text → file** — clipboard text written to the named file.
- **Text → console** — with no destination (or a bare directory), clipboard text is echoed to the terminal.

### Examples

```powershell
clipout shot.png              # Save a clipboard screenshot
clipout shot.jpg --fmt:jpg    # Save, converting the format
clipout diagram.png --b64     # Write the image as a Base64 string
clipout diagram.png --data    # Write the image as an HTML data URI
clipout --llm                 # Extract every file from a bundle into the current dir
clipout .\out\ --llm          # Extract a bundle into a chosen directory
clipout .\dest\               # Paste copied files into a directory
clipout notes.txt             # Write clipboard text to a file
clipout                       # Echo clipboard text to the console
```

## Behavior notes

- **Round-trips with clipin.** A bundle produced by `clipin *.rs --llm` is extracted back to files by `clipout --llm`; matching `--fence:` markers keeps custom fences intact.
- **Directory destinations.** Pointing at a directory writes copied files there, extracts a bundle there, or (for a clipboard image) synthesizes `clipboard.png`.
- **Encoding.** Files are written as clean UTF-8 (no byte-order mark). Line endings in extracted bundle files are normalized to CRLF.

## Compatibility

Windows only. Clipboard access is implemented directly against the Win32 API (`OpenClipboard`, `GetClipboardData`, `DragQueryFileW` for file-drops), so there is no cross-platform build. Clipboard-image extraction shells out to PowerShell's `System.Windows.Forms.Clipboard` for bitmap decoding; the text, file-drop, and bundle paths are fully native.

## License

MIT — see [LICENSE](LICENSE).
