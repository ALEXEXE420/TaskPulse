//! Zeichnet die Anzeige mit Direct2D/DirectWrite in ein 32-Bit-Bitmap und übergibt es per
//! UpdateLayeredWindow (echte Transparenz, auch als Kindfenster der Taskleiste).

use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

pub struct TextItem {
    pub x: f32,
    pub y: f32,
    pub text: String,
}

pub struct Renderer {
    _d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    fmt: Option<(IDWriteTextFormat, String, f32)>,
    memdc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
    bits: *mut u8,
}

impl Renderer {
    pub fn new() -> Result<Renderer> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                // Software: kein D3D-Gerät nötig (spart ~50 MB), für ein kleines Bild schnell genug
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let memdc = CreateCompatibleDC(None);
            Ok(Renderer { _d2d: d2d, dw, rt, fmt: None, memdc, bmp: HBITMAP::default(), old: HGDIOBJ::default(), size: (0, 0), bits: std::ptr::null_mut() })
        }
    }

    pub fn set_font(&mut self, family: &str, px: f32) -> Result<()> {
        if let Some((_, f, p)) = &self.fmt {
            if f == family && (*p - px).abs() < 0.01 {
                return Ok(());
            }
        }
        unsafe {
            let fmt = self.dw.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                &HSTRING::from("de-de"),
            )?;
            fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            self.fmt = Some((fmt, family.to_string(), px));
        }
        Ok(())
    }

    pub fn measure(&self, text: &str) -> f32 {
        let Some((fmt, _, px)) = &self.fmt else { return 0.0 };
        let w: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            match self.dw.CreateTextLayout(&w, fmt, 10000.0, 1000.0) {
                Ok(l) => {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    if l.GetMetrics(&mut m).is_ok() {
                        return m.widthIncludingTrailingWhitespace.ceil();
                    }
                    text.chars().count() as f32 * px * 0.55
                }
                Err(_) => text.chars().count() as f32 * px * 0.55,
            }
        }
    }

    fn ensure_size(&mut self, w: i32, h: i32) -> Result<()> {
        if self.size == (w, h) && !self.bmp.is_invalid() {
            return Ok(());
        }
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.memdc, self.old);
                let _ = DeleteObject(self.bmp);
            }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            self.bmp = CreateDIBSection(self.memdc, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.old = SelectObject(self.memdc, self.bmp);
            self.size = (w, h);
            self.bits = bits as *mut u8;
        }
        Ok(())
    }

    /// Zeichnet die Texte und aktualisiert das Layered-Fenster.
    /// `bg` ist der Hintergrund; Alpha ≥ 1/255 hält das Fenster anklickbar.
    pub fn draw(&mut self, hwnd: HWND, w: i32, h: i32, line_h: f32, items: &[TextItem], seps: &[D2D_RECT_F], fg: D2D1_COLOR_F, bg: D2D1_COLOR_F) -> Result<()> {
        let w = w.max(1);
        let h = h.max(1);
        self.ensure_size(w, h)?;
        let Some((fmt, _, _)) = &self.fmt else { return Ok(()) };
        unsafe {
            let rc = RECT { left: 0, top: 0, right: w, bottom: h };
            self.rt.BindDC(self.memdc, &rc)?;
            self.rt.BeginDraw();
            self.rt.Clear(Some(&bg));
            let base: ID2D1RenderTarget = self.rt.cast()?;
            let brush = base.CreateSolidColorBrush(&fg, None)?;
            if !seps.is_empty() {
                let sc = D2D1_COLOR_F { a: fg.a * 0.45, ..fg };
                let sb = base.CreateSolidColorBrush(&sc, None)?;
                for r in seps {
                    base.FillRectangle(r, &sb);
                }
            }
            for it in items {
                let wide: Vec<u16> = it.text.encode_utf16().collect();
                let r = D2D_RECT_F { left: it.x, top: it.y, right: it.x + 2000.0, bottom: it.y + line_h };
                self.rt.DrawText(&wide, fmt, &r, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            self.rt.EndDraw(None, None)?;
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, None, None, Some(&size), self.memdc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA)?;
        }
        Ok(())
    }
}

impl Renderer {
    /// Speichert das zuletzt gezeichnete Bild als 24-Bit-BMP, auf `bg` (RGB) gemischt (Tests/Doku).
    pub fn save_bmp(&self, path: &std::path::Path, bg: [u8; 3]) -> std::io::Result<()> {
        let (w, h) = self.size;
        if self.bits.is_null() || w <= 0 || h <= 0 {
            return Err(std::io::Error::other("nichts gezeichnet"));
        }
        let _ = unsafe { windows::Win32::Graphics::Gdi::GdiFlush() };
        let src = unsafe { std::slice::from_raw_parts(self.bits, (w * h * 4) as usize) };
        let row = ((w * 3 + 3) / 4 * 4) as usize;
        let mut out = Vec::with_capacity(54 + row * h as usize);
        let size = 54 + row * h as usize;
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        for y in (0..h as usize).rev() {
            let mut line = Vec::with_capacity(row);
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                let (b, g, r, a) = (src[i] as u32, src[i + 1] as u32, src[i + 2] as u32, src[i + 3] as u32);
                // vormultipliziert: out = src + bg * (1 - a)
                let mix = |c: u32, k: u8| (c + k as u32 * (255 - a) / 255).min(255) as u8;
                line.push(mix(b, bg[2]));
                line.push(mix(g, bg[1]));
                line.push(mix(r, bg[0]));
            }
            line.resize(row, 0);
            out.extend_from_slice(&line);
        }
        std::fs::write(path, out)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.memdc, self.old);
                let _ = DeleteObject(self.bmp);
            }
            let _ = DeleteDC(self.memdc);
        }
    }
}

/// Farbe mit unvormultipliziertem Alpha (Direct2D rechnet selbst um).
pub fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
