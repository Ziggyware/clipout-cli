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
