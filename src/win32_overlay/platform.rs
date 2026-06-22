use std::ffi::c_void;
use std::mem::MaybeUninit;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DeleteObject, FF_DONTCARE,
    FIXED_PITCH, FW_NORMAL, GetDC, HBITMAP, HBRUSH, HFONT, HGDIOBJ, OUT_TT_PRECIS, ReleaseDC,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, HICON, HWND_TOPMOST, IDC_ARROW, LoadCursorW,
    MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA,
    UpdateLayeredWindow, WM_HOTKEY, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::geometry::{ScreenRect, Size};

use super::{CLASS_NAME, Canvas, FONT_FACE_MONO, FONT_FACE_UI, OverlayEvent, WINDOW_TITLE};

pub(super) fn present_layered(hwnd: HWND, screen_rect: ScreenRect, canvas: &Canvas) {
    let pos = POINT {
        x: screen_rect.x,
        y: screen_rect.y,
    };
    let size = SIZE {
        cx: canvas.width,
        cy: canvas.height,
    };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    unsafe {
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&pos),
            Some(&size),
            Some(canvas.dc),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn create_canvas(width: i32, height: i32) -> Option<Canvas> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    unsafe {
        let screen = GetDC(None);
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let _ = ReleaseDC(None, screen);
        let _ = &mut info;
        let bitmap: HBITMAP = bitmap.ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            return None;
        }
        let dc = CreateCompatibleDC(None);
        SelectObject(dc, HGDIOBJ(bitmap.0));
        Some(Canvas {
            dc,
            bitmap,
            bits: bits as *mut u8,
            width,
            height,
        })
    }
}

pub(super) fn create_font(height: i32, weight: i32) -> HFONT {
    let face = wide(FONT_FACE_UI);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

/// Proportional UI font with *grayscale* antialiasing rather than ClearType.
///
/// The always-on overlay furigana is drawn straight onto the transparent
/// layered surface, where its per-pixel alpha is recovered from the glyph's
/// grey coverage (see `draw_overlay_glyphs`). ClearType's subpixel colour
/// fringing would make the three channels disagree and corrupt that recovery, so
/// grayscale AA (equal channels == coverage) is required here.
pub(super) fn create_aa_font(height: i32, weight: i32) -> HFONT {
    let face = wide(FONT_FACE_UI);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

/// Fixed-pitch font for the HUD so numeric columns align. Falls back to GDI's
/// default fixed-pitch face if Consolas is unavailable.
pub(super) fn create_mono_font(height: i32) -> HFONT {
    let face = wide(FONT_FACE_MONO);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (FIXED_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

pub(super) fn create_overlay_window() -> Result<HWND> {
    let class_name = wide(CLASS_NAME);
    let title = wide(WINDOW_TITLE);
    let instance = current_instance()?;
    register_overlay_class(instance, &class_name);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )
        .context("failed to create overlay window")?
    };

    // No SetLayeredWindowAttributes: UpdateLayeredWindow drives per-pixel alpha.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    Ok(hwnd)
}

fn register_overlay_class(instance: HINSTANCE, class_name: &[u16]) {
    static REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REGISTERED.get_or_init(|| {
        // Standard arrow cursor: without it the class cursor is null, so when
        // the overlay is interactive (not click-through, e.g. over the
        // problem-report button) Windows falls back to the busy/wait cursor.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
        let class = WNDCLASSW {
            style: Default::default(),
            lpfnWndProc: Some(overlay_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: HICON::default(),
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
        };
        unsafe {
            RegisterClassW(&class);
        }
    });
}

fn current_instance() -> Result<HINSTANCE> {
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.context("failed to get module")?;
    Ok(HINSTANCE(module.0))
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

pub(super) fn pump_messages() -> Vec<OverlayEvent> {
    let mut events = Vec::new();
    unsafe {
        loop {
            let mut msg = MaybeUninit::<MSG>::zeroed();
            if !PeekMessageW(msg.as_mut_ptr(), None, 0, 0, PM_REMOVE).as_bool() {
                break;
            }
            let msg = msg.assume_init();
            if msg.message == WM_HOTKEY {
                if let Ok(id) = i32::try_from(msg.wParam.0) {
                    events.push(OverlayEvent::Hotkey(id));
                }
                continue;
            }
            if msg.message == WM_LBUTTONDOWN {
                let (x, y) = mouse_coords(msg.lParam);
                events.push(OverlayEvent::LeftButtonDown { x, y });
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    events
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn mouse_coords(lparam: LPARAM) -> (i32, i32) {
    let raw = lparam.0 as u32;
    let x = (raw & 0xffff) as i16 as i32;
    let y = ((raw >> 16) & 0xffff) as i16 as i32;
    (x, y)
}

pub(super) fn u32_to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

impl Default for ScreenRect {
    fn default() -> Self {
        Self::new(0, 0, Size::new(1, 1))
    }
}
