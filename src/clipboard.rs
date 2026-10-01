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
