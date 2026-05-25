//! Win32 splash window for the `mai` launcher (WELCOME-01).
//!
//! Shows a centered, borderless, always-on-top window displaying the
//! Lamprey MAI gold-badge logo for a fixed duration, then closes. The
//! image bytes are baked into the binary via `include_bytes!` so the
//! launcher is self-contained — no asset files are read at runtime.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use image::GenericImageView;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, GetObjectW, HBITMAP, HDC, HGDIOBJ,
    PAINTSTRUCT, SRCCOPY, SelectObject, StretchBlt,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetSystemMetrics, IDC_ARROW, KillTimer, LoadCursorW, MSG,
    PostQuitMessage, RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SetTimer, ShowWindow,
    TranslateMessage, WM_DESTROY, WM_LBUTTONDOWN, WM_PAINT, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

const LAMPREY_LOGO_PNG: &[u8] = include_bytes!("../../../docs/assets/lamprey-mai-logo.png");

const SPLASH_MAX_DIM: i32 = 600;
const SPLASH_TIMER_ID: usize = 1;

static SPLASH_BITMAP: OnceLock<isize> = OnceLock::new();

pub fn show_splash(duration_ms: u32) -> Result<()> {
    let img =
        image::load_from_memory(LAMPREY_LOGO_PNG).context("decode embedded lamprey logo PNG")?;
    let (bmp_w, bmp_h) = img.dimensions();
    let (target_w, target_h) = fit_into_box(bmp_w as i32, bmp_h as i32, SPLASH_MAX_DIM);
    let rgba = img.to_rgba8();

    let h_instance: HINSTANCE =
        unsafe { GetModuleHandleW(None).context("GetModuleHandleW")?.into() };
    let class_name = w!("LampreyMaiSplash");
    register_class(h_instance, class_name)?;

    let hbmp = create_dib_from_rgba(&rgba, bmp_w, bmp_h)?;
    SPLASH_BITMAP
        .set(hbmp.0 as isize)
        .map_err(|_| anyhow!("splash bitmap already initialised"))?;

    let (x, y) = center_on_primary(target_w, target_h);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name,
            &HSTRING::from("Lamprey MAI"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            target_w,
            target_h,
            None,
            None,
            h_instance,
            None,
        )
        .context("CreateWindowExW (splash)")?
    };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        if SetTimer(hwnd, SPLASH_TIMER_ID, duration_ms, None) == 0 {
            let _ = DestroyWindow(hwnd);
            return Err(anyhow!("SetTimer failed"));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let gdi: HGDIOBJ = HBITMAP(hbmp.0).into();
        let _ = DeleteObject(gdi);
    }
    Ok(())
}

fn fit_into_box(src_w: i32, src_h: i32, max_dim: i32) -> (i32, i32) {
    if src_w <= 0 || src_h <= 0 {
        return (max_dim, max_dim);
    }
    let scale = (f64::from(max_dim) / f64::from(src_w.max(src_h))).min(1.0);
    let w = (f64::from(src_w) * scale).round() as i32;
    let h = (f64::from(src_h) * scale).round() as i32;
    (w.max(1), h.max(1))
}

fn center_on_primary(w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        ((sw - w) / 2, (sh - h) / 2)
    }
}

fn register_class(h_instance: HINSTANCE, class_name: PCWSTR) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW).context("LoadCursorW IDC_ARROW")? };
    let wnd_class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(0),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: h_instance,
        hCursor: cursor,
        lpszClassName: class_name,
        ..Default::default()
    };
    unsafe {
        // Returns 0 on failure; "already registered" returns the
        // existing atom which is fine for re-entry.
        RegisterClassExW(&wnd_class);
    }
    Ok(())
}

fn create_dib_from_rgba(rgba: &[u8], w: u32, h: u32) -> Result<HBITMAP> {
    let header = BITMAPINFOHEADER {
        biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(0),
        biWidth: i32::try_from(w).context("image width too large for BITMAPINFOHEADER")?,
        biHeight: -i32::try_from(h).context("image height too large for BITMAPINFOHEADER")?,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };

    let mut bits_ptr: *mut c_void = ptr::null_mut();
    let hbmp = unsafe {
        CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits_ptr, None, 0)
            .context("CreateDIBSection")?
    };
    if bits_ptr.is_null() {
        unsafe {
            let gdi: HGDIOBJ = hbmp.into();
            let _ = DeleteObject(gdi);
        }
        return Err(anyhow!("CreateDIBSection returned null bits pointer"));
    }

    unsafe {
        let dst = std::slice::from_raw_parts_mut(bits_ptr.cast::<u8>(), rgba.len());
        for (chunk_in, chunk_out) in rgba.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            chunk_out[0] = chunk_in[2]; // B
            chunk_out[1] = chunk_in[1]; // G
            chunk_out[2] = chunk_in[0]; // R
            chunk_out[3] = chunk_in[3]; // A
        }
    }

    Ok(hbmp)
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                paint_splash(hwnd);
                LRESULT(0)
            }
            WM_TIMER | WM_LBUTTONDOWN => {
                let _ = KillTimer(hwnd, SPLASH_TIMER_ID);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn paint_splash(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc: HDC = BeginPaint(hwnd, &mut ps);

        if let Some(handle) = SPLASH_BITMAP.get() {
            let hbmp = HBITMAP(*handle as *mut c_void);
            let mem_dc = CreateCompatibleDC(hdc);
            let bmp_gdi: HGDIOBJ = hbmp.into();
            let old = SelectObject(mem_dc, bmp_gdi);

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);

            let mut bmp = BITMAP::default();
            let bmp_ptr: *mut c_void = (&raw mut bmp).cast();
            let bmp_for_info: HGDIOBJ = hbmp.into();
            GetObjectW(
                bmp_for_info,
                i32::try_from(size_of::<BITMAP>()).unwrap_or(0),
                Some(bmp_ptr),
            );

            let _ = StretchBlt(
                hdc,
                0,
                0,
                rect.right,
                rect.bottom,
                mem_dc,
                0,
                0,
                bmp.bmWidth,
                bmp.bmHeight,
                SRCCOPY,
            );

            SelectObject(mem_dc, old);
            let _ = DeleteDC(mem_dc);
        }

        let _ = EndPaint(hwnd, &ps);
    }
}
