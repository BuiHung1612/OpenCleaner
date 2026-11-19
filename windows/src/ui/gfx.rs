//! Painting. Shapes go straight to GDI+ (anti-aliased, with alpha, for the glass panels);
//! text is collected and drawn last with GDI (ClearType), each run clipped to its area.
//! Everything is in logical pixels; `s` is the window's DPI scale.

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::*;

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> R {
        R::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersects(&self, o: &R) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// 0xRRGGBB with an alpha, as GDI+ wants it (0xAARRGGBB).
pub fn argb(rgb: u32, alpha: u8) -> u32 {
    (u32::from(alpha) << 24) | (rgb & 0x00FF_FFFF)
}

pub fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0x555555)
}

pub const WHITE: u32 = 0xFFFFFF;

/// The system's app theme (Settings > Personalization > Colors). The dark look is the
/// original one: white text on deep gradients; the light one pales the gradients and
/// turns the text dark.
static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

pub fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

/// Text, icons and the tints of hovered or picked rows.
pub fn fg() -> u32 {
    if light() { 0x1B2333 } else { WHITE }
}

/// Secondary text: white at about 78% on the dark gradients.
pub fn dim() -> u32 {
    if light() { 0x4A5468 } else { 0xD4DAE6 }
}

pub fn amber() -> u32 {
    if light() { 0xA35A00 } else { 0xFFD54F }
}

pub fn mint() -> u32 {
    if light() { 0x0A7F58 } else { 0x7CF2C8 }
}

/// A page color as the background of the current theme (paled towards white when light).
pub fn backdrop(rgb: u32) -> u32 {
    if !light() {
        return rgb;
    }
    let mix = |shift: u32| {
        let c = (rgb >> shift & 0xFF) as f32;
        ((c * 0.22 + 255.0 * 0.78) as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

struct TextRun {
    r: R,
    clip: Option<R>,
    text: String,
    size: f32,
    bold: bool,
    color: u32,
    align: Align,
    wrap: bool,
    icon: bool,
}

pub struct Canvas {
    g: *mut GpGraphics,
    hdc: HDC,
    pub s: f32,
    clip: Option<R>,
    texts: Vec<TextRun>,
}

impl Canvas {
    pub fn new(hdc: HDC, s: f32) -> Canvas {
        let mut g = null_mut();
        unsafe {
            GdipCreateFromHDC(hdc, &mut g);
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        }
        Canvas { g, hdc, s, clip: None, texts: Vec::new() }
    }

    /// Clips shapes and text to `r` until `unclip`.
    pub fn clip(&mut self, r: R) {
        self.clip = Some(r);
        let s = self.s;
        unsafe { GdipSetClipRect(self.g, r.x * s, r.y * s, r.w * s, r.h * s, CombineModeReplace) };
    }

    pub fn unclip(&mut self) {
        self.clip = None;
        unsafe { GdipResetClip(self.g) };
    }

    fn brush(&self, color: u32) -> *mut GpBrush {
        let mut b = null_mut();
        unsafe { GdipCreateSolidFill(color, &mut b) };
        b as *mut GpBrush
    }

    /// Diagonal two-color gradient, top-left to bottom-right.
    pub fn gradient(&self, r: R, from: u32, to: u32) {
        let s = self.s;
        let a = PointF { X: r.x * s, Y: r.y * s };
        let b = PointF { X: r.right() * s, Y: r.bottom() * s };
        unsafe {
            let mut brush = null_mut();
            GdipCreateLineBrush(&a, &b, argb(from, 255), argb(to, 255), WrapModeTileFlipXY, &mut brush);
            GdipFillRectangle(self.g, brush as *mut GpBrush, r.x * s, r.y * s, r.w * s, r.h * s);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
    }

    pub fn rounded(&self, r: R, radius: f32, fill: Option<u32>, stroke: Option<u32>) {
        let s = self.s;
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let d = (radius * 2.0 * s).min(h).min(w).max(1.0);
        unsafe {
            let mut path = null_mut();
            GdipCreatePath(FillModeAlternate, &mut path);
            GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
            GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(path);
            if let Some(c) = fill {
                let b = self.brush(c);
                GdipFillPath(self.g, b, path);
                GdipDeleteBrush(b);
            }
            if let Some(c) = stroke {
                let mut pen = null_mut();
                GdipCreatePen1(c, s.max(1.0), UnitPixel, &mut pen);
                GdipDrawPath(self.g, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    /// The one glass surface for every panel (white tint over the gradient).
    pub fn glass(&self, r: R, radius: f32) {
        if light() {
            self.rounded(r, radius, Some(argb(WHITE, 150)), Some(argb(0, 28)));
        } else {
            self.rounded(r, radius, Some(argb(WHITE, 26)), Some(argb(WHITE, 56)));
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, fill: Option<u32>, stroke: Option<(u32, f32)>) {
        let s = self.s;
        let (x, y, d) = ((cx - radius) * s, (cy - radius) * s, 2.0 * radius * s);
        unsafe {
            if let Some(c) = fill {
                let b = self.brush(c);
                GdipFillEllipse(self.g, b, x, y, d, d);
                GdipDeleteBrush(b);
            }
            if let Some((c, width)) = stroke {
                let mut pen = null_mut();
                GdipCreatePen1(c, width * s, UnitPixel, &mut pen);
                GdipDrawEllipse(self.g, pen, x, y, d, d);
                GdipDeletePen(pen);
            }
        }
    }

    /// An arc with round ends (health ring, busy spinner). Angles in degrees, 0 = 3 o'clock.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, width: f32, start: f32, sweep: f32, color: u32) {
        let s = self.s;
        unsafe {
            let mut pen = null_mut();
            GdipCreatePen1(color, width * s, UnitPixel, &mut pen);
            GdipSetPenStartCap(pen, LineCapRound);
            GdipSetPenEndCap(pen, LineCapRound);
            GdipDrawArc(self.g, pen, (cx - radius) * s, (cy - radius) * s, 2.0 * radius * s, 2.0 * radius * s, start, sweep);
            GdipDeletePen(pen);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_text(&mut self, r: R, text: &str, size: f32, bold: bool, color: u32, align: Align, wrap: bool, icon: bool) {
        if text.is_empty() {
            return;
        }
        self.texts.push(TextRun { r, clip: self.clip, text: text.into(), size, bold, color, align, wrap, icon });
    }

    /// One line, ellipsis when it does not fit, vertically centered in `r`.
    pub fn text(&mut self, r: R, text: &str, size: f32, bold: bool, color: u32, align: Align) {
        self.push_text(r, text, size, bold, color, align, false, false);
    }

    /// Wrapped paragraph from the top of `r`.
    pub fn para(&mut self, r: R, text: &str, size: f32, color: u32) {
        self.push_text(r, text, size, false, color, Align::Left, true, false);
    }

    /// A Segoe Fluent Icons / MDL2 glyph, centered in `r`.
    pub fn icon(&mut self, r: R, glyph: char, size: f32, color: u32) {
        self.push_text(r, &glyph.to_string(), size, false, color, Align::Center, false, true);
    }

    /// Draws the collected text and releases GDI+.
    pub fn finish(mut self, fonts: &mut Fonts) {
        unsafe {
            GdipFlush(self.g, FlushIntentionFlush);
            GdipDeleteGraphics(self.g);
            SetBkMode(self.hdc, TRANSPARENT as i32);
        }
        let s = self.s;
        for t in std::mem::take(&mut self.texts) {
            let px = |v: f32| (v * s).round() as i32;
            let mut rect = RECT { left: px(t.r.x), top: px(t.r.y), right: px(t.r.right()), bottom: px(t.r.bottom()) };
            unsafe {
                if let Some(c) = t.clip {
                    let rgn = CreateRectRgn(px(c.x), px(c.y), px(c.right()), px(c.bottom()));
                    SelectClipRgn(self.hdc, rgn);
                    DeleteObject(rgn);
                } else {
                    SelectClipRgn(self.hdc, null_mut());
                }
                SelectObject(self.hdc, fonts.get(t.size * s, t.bold, t.icon));
                SetTextColor(self.hdc, (t.color & 0xFF) << 16 | (t.color & 0xFF00) | (t.color >> 16 & 0xFF));
                let mut w: Vec<u16> = t.text.encode_utf16().collect();
                let align = match t.align {
                    Align::Left => DT_LEFT,
                    Align::Center => DT_CENTER,
                    Align::Right => DT_RIGHT,
                };
                let flow = if t.wrap { DT_WORDBREAK } else { DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS };
                DrawTextW(self.hdc, w.as_mut_ptr(), w.len() as i32, &mut rect, flow | DT_NOPREFIX | align);
            }
        }
        unsafe { SelectClipRgn(self.hdc, null_mut()) };
    }
}

/// GDI fonts by pixel size, weight and family, kept for the life of the window.
#[derive(Default)]
pub struct Fonts(HashMap<(i32, bool, bool), HFONT>);

impl Fonts {
    pub fn get(&mut self, px: f32, bold: bool, icon: bool) -> HFONT {
        let key = (px.round() as i32, bold, icon);
        *self.0.entry(key).or_insert_with(|| {
            let face: Vec<u16> = if icon {
                // Windows 11 ships Segoe Fluent Icons; Windows 10 the same glyphs in MDL2 Assets
                if font_installed("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }
            } else {
                "Segoe UI Variable Text"
            }
            .encode_utf16()
            .chain([0])
            .collect();
            unsafe {
                CreateFontW(
                    -key.0,
                    0,
                    0,
                    0,
                    if bold { FW_SEMIBOLD as i32 } else { FW_NORMAL as i32 },
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32,
                    DEFAULT_PITCH as u32,
                    face.as_ptr(),
                )
            }
        })
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in self.0.values() {
            unsafe { DeleteObject(*f) };
        }
    }
}

fn font_installed(name: &str) -> bool {
    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, flag: isize) -> i32 {
        unsafe { *(flag as *mut bool) = true };
        0
    }
    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    for (i, c) in name.encode_utf16().take(31).enumerate() {
        lf.lfFaceName[i] = c;
    }
    let mut yes = false;
    unsafe {
        let hdc = GetDC(null_mut());
        EnumFontFamiliesExW(hdc, &lf, Some(found), &mut yes as *mut bool as isize, 0);
        ReleaseDC(null_mut(), hdc);
    }
    yes
}

/// Starts GDI+ for the life of the process.
pub fn startup() {
    let mut token = 0usize;
    let input = GdiplusStartupInput { GdiplusVersion: 1, ..unsafe { std::mem::zeroed() } };
    unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
    let _ = null::<u8>();
}
