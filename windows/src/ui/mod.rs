//! The window: a gradient that changes with the page, a glass sidebar, glass panels, the big
//! round button, pill buttons. Win32 has none of these controls, so the whole client area
//! is painted (double-buffered) and clicks are matched against the areas the last paint
//! registered. Long engine work runs on threads (`jobs`) and comes back as messages.

mod auto;
mod gfx;
mod jobs;
mod pages;

use gfx::Canvas;
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub use pages::State;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the window state, unless it is already in use (a modal dialog's message loop
/// can call back into the window while an action is running).
pub fn with<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Where the app keeps its settings (HKEY_CURRENT_USER).
pub const SETTINGS_KEY: &str = r"Software\OpenCleaner";

/// A DWORD under HKEY_CURRENT_USER\`key`, if it is set.
pub fn reg_dword(key: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let (key, name) = (wide(key), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut value as *mut u32 as *mut _, &mut size)
    };
    (status == ERROR_SUCCESS).then_some(value)
}

/// Sets a DWORD under HKEY_CURRENT_USER\`key`, creating the key.
pub fn set_reg_dword(key: &str, name: &str, value: u32) {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RegSetKeyValueW};
    let (key, name) = (wide(key), wide(name));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_DWORD, &value as *const u32 as *const _, 4) };
}

/// Reads the system's app theme (missing before Windows 10 1809: dark, the original look).
fn read_theme() {
    gfx::read_motion();
    gfx::set_light(reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(1));
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    gfx::startup();
    read_theme();
    let class = wide("OpenCleanerWindow");
    let title = wide("OpenCleaner");
    unsafe {
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            // MAKEINTRESOURCE(1): the icon winresource embeds
            hIcon: LoadIconW(instance, std::ptr::without_provenance::<u16>(1)),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1080,
            720,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        // logical size 1080 x 720 at this monitor's DPI
        let s = GetDpiForWindow(hwnd) as f32 / 96.0;
        SetWindowPos(hwnd, null_mut(), 0, 0, (1080.0 * s) as i32, (720.0 * s) as i32, SWP_NOMOVE | SWP_NOZORDER);
        STATE.with(|cell| *cell.borrow_mut() = Some(State::new(hwnd)));
        style_frame(hwnd, pages::Module::Dashboard.palette().0);
        ShowWindow(hwnd, SW_SHOW);
        with(|st| st.on_show_module());
        // scheduled cleaning: now, then every hour
        with(|st| st.auto_clean_check());
        SetTimer(hwnd, TIMER_AUTO, 3_600_000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Rounded frame whose title bar takes the page's color, in the system theme (Windows 11;
/// ignored before).
pub fn style_frame(hwnd: HWND, caption: u32) {
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;
    let set = |attr: u32, value: u32| unsafe {
        DwmSetWindowAttribute(hwnd, attr, &value as *const u32 as *const _, 4);
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(!gfx::light()));
    set(DWMWA_WINDOW_CORNER_PREFERENCE, 2);
    let bgr = |c: u32| (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16 & 0xFF);
    set(DWMWA_CAPTION_COLOR, bgr(gfx::backdrop(caption)));
    set(DWMWA_TEXT_COLOR, bgr(gfx::fg()));
}

pub fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

pub fn invalidate(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

/// Not in the windows-sys modules used here; a bare name in a `match` would catch everything.
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_SPIN: usize = 1;
/// Hourly: is a scheduled clean due?
const TIMER_AUTO: usize = 2;
/// Runs while something moves across the window (page change, meters, bubbles): ~60 fps.
const TIMER_ANIM: usize = 4;

pub fn anim_timer(hwnd: HWND, on: bool) {
    unsafe {
        if on {
            SetTimer(hwnd, TIMER_ANIM, 16, None);
        } else {
            KillTimer(hwnd, TIMER_ANIM);
        }
    }
}

/// Settings > Dashboard > Refresh automatically, while the dashboard is on screen.
const TIMER_DASH: usize = 3;

/// Starts (every `secs` seconds) or stops the dashboard's refresh timer.
pub fn dash_timer(hwnd: HWND, secs: Option<u32>) {
    unsafe {
        match secs {
            Some(s) => SetTimer(hwnd, TIMER_DASH, s * 1000, None),
            None => KillTimer(hwnd, TIMER_DASH) as usize,
        };
    }
}

/// The folder chooser; `None` when cancelled.
pub fn pick_folder(hwnd: HWND, title: &str) -> Option<String> {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::{BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW};
    let title = wide(title);
    let mut name = [0u16; 260];
    unsafe {
        // the resizable dialog needs COM on this thread (a second call is harmless)
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        let info = BROWSEINFOW {
            hwndOwner: hwnd,
            pidlRoot: null_mut(),
            pszDisplayName: name.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: 0,
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&info);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 1024];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr()) != 0;
        ILFree(pidl);
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&path[..len]))
    }
}

/// Keeps a timer running while something animates (busy buttons). The dashboard has no
/// timer: it reads once, then on the refresh button (or on focus, if chosen in Settings).
pub fn timers(hwnd: HWND, spinning: bool) {
    unsafe {
        if spinning {
            SetTimer(hwnd, TIMER_SPIN, 40, None);
        } else {
            KillTimer(hwnd, TIMER_SPIN);
        }
    }
}

/// Kept between paints: the back buffer, and the backdrop (gradient, light blobs, sidebar
/// glass), redrawn only when the page colors, theme, size or DPI change.
struct Buffers {
    back: HDC,
    back_bmp: HBITMAP,
    bg: HDC,
    bg_bmp: HBITMAP,
    size: (i32, i32),
    /// (page colors, light theme, scale bits) the backdrop was drawn for
    key: Option<((u32, u32), bool, u32)>,
    /// the last frame of the previous page, faded out over the new one (like the Mac's
    /// 0.35 s cross-fade)
    prev: HDC,
    prev_bmp: HBITMAP,
    page: Option<pages::Module>,
    fade: Option<std::time::Instant>,
}

const FADE_SECS: f32 = 0.35;

thread_local! {
    static BUFFERS: RefCell<Option<Buffers>> = const { RefCell::new(None) };
}

/// Paints only the invalidated area: the backdrop is copied from its cache, the page is drawn
/// clipped to that area (so a spinner tick repaints a few dozen pixels, not the window).
fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right.max(1), rc.bottom.max(1));
        let d = ps.rcPaint;
        let s = scale(hwnd);
        BUFFERS.with(|cell| {
            let Ok(mut b) = cell.try_borrow_mut() else { return };
            let b = b.get_or_insert_with(|| Buffers {
                back: CreateCompatibleDC(hdc),
                back_bmp: null_mut(),
                bg: CreateCompatibleDC(hdc),
                bg_bmp: null_mut(),
                size: (0, 0),
                key: None,
                prev: CreateCompatibleDC(hdc),
                prev_bmp: null_mut(),
                page: None,
                fade: None,
            });
            if b.size != (w, h) {
                for (dc, bmp) in [(b.back, &mut b.back_bmp), (b.bg, &mut b.bg_bmp), (b.prev, &mut b.prev_bmp)] {
                    let new = CreateCompatibleBitmap(hdc, w, h);
                    SelectObject(dc, new);
                    if !bmp.is_null() {
                        DeleteObject(*bmp);
                    }
                    *bmp = new;
                }
                b.size = (w, h);
                b.key = None;
            }
            let (lw, lh) = (w as f32 / s, h as f32 / s);
            if let Some(colors) = with(|st| st.palette()) {
                let key = (colors, gfx::light(), s.to_bits());
                if b.key != Some(key) {
                    let c = Canvas::new(b.bg, s, None);
                    pages::draw_backdrop(&c, colors, lw, lh);
                    c.finish(&mut gfx::Fonts::default());
                    b.key = Some(key);
                }
            }
            // a new page: keep the old one's last frame to fade out (`back` still holds it)
            let page = with(|st| st.module());
            if page != b.page {
                if b.page.is_some() && gfx::motion() {
                    BitBlt(b.prev, 0, 0, w, h, b.back, 0, 0, SRCCOPY);
                    b.fade = Some(std::time::Instant::now());
                    with(|st| st.animate(FADE_SECS));
                }
                b.page = page;
            }
            let (dw, dh) = (d.right - d.left, d.bottom - d.top);
            BitBlt(b.back, d.left, d.top, dw, dh, b.bg, d.left, d.top, SRCCOPY);
            let drawn = with(|st| {
                let mut c = Canvas::new(b.back, s, Some(d));
                st.draw(&mut c, lw, lh);
                (c, std::mem::take(&mut st.fonts))
            });
            if let Some((c, mut fonts)) = drawn {
                c.finish(&mut fonts);
                with(|st| st.fonts = fonts);
            }
            BitBlt(hdc, d.left, d.top, dw, dh, b.back, d.left, d.top, SRCCOPY);
            if let Some(start) = b.fade {
                let t = start.elapsed().as_secs_f32() / FADE_SECS;
                if t < 1.0 {
                    let alpha = (255.0 * (1.0 - gfx::ease_in_out(t))) as u8;
                    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
                    AlphaBlend(hdc, d.left, d.top, dw, dh, b.prev, d.left, d.top, dw, dh, blend);
                } else {
                    b.fade = None;
                }
            }
        });
        EndPaint(hwnd, &ps);
    }
}

fn mouse(lparam: LPARAM, hwnd: HWND) -> (f32, f32) {
    let s = scale(hwnd);
    let x = (lparam & 0xFFFF) as i16 as f32;
    let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
    (x / s, y / s)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            invalidate(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            let info = lparam as *mut MINMAXINFO;
            let s = scale(hwnd);
            unsafe {
                (*info).ptMinTrackSize.x = (980.0 * s) as i32;
                (*info).ptMinTrackSize.y = (640.0 * s) as i32;
            }
            0
        }
        WM_DPICHANGED => {
            let r = lparam as *const RECT;
            unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    (*r).left,
                    (*r).top,
                    (*r).right - (*r).left,
                    (*r).bottom - (*r).top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            invalidate(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse(lparam, hwnd);
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe { TrackMouseEvent(&mut tme) };
            if with(|st| st.hover_at(x, y)).unwrap_or(false) {
                invalidate(hwnd);
            }
            0
        }
        WM_MOUSELEAVE => {
            if with(|st| st.hover_at(-1.0, -1.0)).unwrap_or(false) {
                invalidate(hwnd);
            }
            0
        }
        WM_SETCURSOR => {
            if (lparam & 0xFFFF) as u32 == HTCLIENT && with(|st| st.hovering_action()).unwrap_or(false) {
                unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
                return 1;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse(lparam, hwnd);
            if let Some(Some(action)) = with(|st| st.action_at(x, y)) {
                pages::perform(hwnd, action);
                invalidate(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
            let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
            let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
            if let Some(Some(action)) = with(|st| st.key(wparam as u16, ctrl, shift)) {
                pages::perform(hwnd, action);
            }
            invalidate(hwnd);
            0
        }
        WM_SETTINGCHANGE => {
            // the app theme may have changed (lParam "ImmersiveColorSet"); reading it is cheap
            read_theme();
            if let Some(caption) = with(|st| st.caption()) {
                style_frame(hwnd, caption);
            }
            invalidate(hwnd);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as f32;
            with(|st| st.scroll_by(-delta / 120.0 * 60.0));
            invalidate(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_ANIM => {
            if with(|st| st.animating()) != Some(true) {
                anim_timer(hwnd, false);
            }
            invalidate(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_DASH => {
            // minimized: nothing to show, skip the reading
            if unsafe { IsIconic(hwnd) } == 0 {
                with(|st| st.refresh_dashboard());
            }
            0
        }
        WM_TIMER if wparam == TIMER_AUTO => {
            with(|st| st.auto_clean_check());
            0
        }
        WM_TIMER => {
            // only the spinners move: repaint their squares
            let s = scale(hwnd);
            for r in with(|st| st.spinners()).unwrap_or_default() {
                let px = |v: f32| (v * s).round() as i32;
                let rect = RECT { left: px(r.x), top: px(r.y), right: px(r.right()), bottom: px(r.bottom()) };
                unsafe { InvalidateRect(hwnd, &rect, 0) };
            }
            0
        }
        WM_ACTIVATE => {
            // low word: WA_INACTIVE (0) when another window takes the focus
            if wparam & 0xFFFF != 0 {
                with(|st| st.on_activate());
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        jobs::WM_ENGINE => {
            let posted = unsafe { Box::from_raw(lparam as *mut jobs::Posted) };
            with(|st| st.on_event(posted.0, posted.1));
            invalidate(hwnd);
            0
        }
        WM_DESTROY => {
            auto::remove_tray(hwnd);
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Native confirmation (OK / Cancel) for anything that removes files.
pub fn confirm(hwnd: HWND, text: &str) -> bool {
    let text = wide(text);
    let caption = wide("OpenCleaner");
    unsafe { MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) == IDOK }
}

/// Opens Explorer on `path` (its folder, with it selected).
pub fn reveal(path: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
}
