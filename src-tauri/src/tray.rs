//! Tray icon rendering and popup placement.

use crate::core::Snapshot;
use crate::i18n;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tauri::image::Image;
use tauri::{AppHandle, Manager, PhysicalPosition, Rect, Runtime, WebviewWindow};

static FONT: &[u8] = include_bytes!("../assets/digits.ttf");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Normal,
    Charging,
    Low,
    Dim,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TrayLook {
    pub text: String,
    pub tone: Tone,
    pub fill: Option<u8>,
    pub light_bar: bool,
}

pub fn look(s: &Snapshot, low_threshold: u8) -> TrayLook {
    let light_bar = system_uses_light_theme();
    match s.soc {
        Some(soc) if !s.stale => TrayLook {
            text: soc.to_string(),
            tone: if s.charging == Some(true) {
                Tone::Charging
            } else if soc <= low_threshold {
                Tone::Low
            } else {
                Tone::Normal
            },
            fill: Some(soc),
            light_bar,
        },
        Some(soc) => TrayLook { text: soc.to_string(), tone: Tone::Dim, fill: Some(soc), light_bar },
        None => TrayLook { text: "–".into(), tone: Tone::Dim, fill: None, light_bar },
    }
}

#[cfg(windows)]
fn system_uses_light_theme() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|v| v == 1)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn system_uses_light_theme() -> bool {
    false
}

fn color(l: &TrayLook) -> [u8; 3] {
    match (l.tone, l.light_bar) {
        (Tone::Charging, false) => [0x3D, 0xDC, 0x97],
        (Tone::Charging, true) => [0x0F, 0x8A, 0x5F],
        (Tone::Low, false) => [0xFF, 0x6B, 0x63],
        (Tone::Low, true) => [0xC6, 0x28, 0x28],
        (Tone::Normal, false) => [0xFF, 0xFF, 0xFF],
        (Tone::Normal, true) => [0x1A, 0x1A, 0x1A],
        (Tone::Dim, false) => [0x9A, 0x9A, 0x9A],
        (Tone::Dim, true) => [0x70, 0x70, 0x70],
    }
}

/// Windows / Linux: the number itself is the icon (32×32).
pub fn render_number(l: &TrayLook) -> Image<'static> {
    const S: u32 = 32;
    let mut buf = vec![0u8; (S * S * 4) as usize];
    let font = FontRef::try_from_slice(FONT).expect("font");
    let [r, g, b] = color(l);

    // Pick the biggest size that fits the width.
    let mut px = 30.0f32;
    let width_at = |px: f32| {
        let sf = font.as_scaled(PxScale::from(px));
        l.text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum::<f32>()
    };
    while px > 10.0 && width_at(px) > (S as f32) - 1.0 {
        px -= 0.5;
    }
    let sf = font.as_scaled(PxScale::from(px));
    let total_w = width_at(px);
    let ascent = sf.ascent();
    // Digits have no descenders: centre the cap height.
    let cap = px * 0.73;
    let baseline = ((S as f32) + cap) / 2.0;
    let mut x = ((S as f32) - total_w) / 2.0;

    for ch in l.text.chars() {
        let id = sf.glyph_id(ch);
        let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
        x += sf.h_advance(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let px_x = bb.min.x as i32 + gx as i32;
                let px_y = bb.min.y as i32 + gy as i32;
                if px_x < 0 || px_y < 0 || px_x >= S as i32 || px_y >= S as i32 {
                    return;
                }
                let i = ((px_y as u32 * S + px_x as u32) * 4) as usize;
                let a = (cov.min(1.0) * 255.0) as u8;
                if a > buf[i + 3] {
                    buf[i] = r;
                    buf[i + 1] = g;
                    buf[i + 2] = b;
                    buf[i + 3] = a;
                }
            });
        }
    }
    let _ = ascent;
    Image::new_owned(buf, S, S)
}

/// macOS: a template battery glyph filled to the level; the number goes into the title.
pub fn render_battery_template(fill: Option<u8>) -> Image<'static> {
    const W: u32 = 44;
    const H: u32 = 44;
    let mut buf = vec![0u8; (W * H * 4) as usize];
    let mut put = |x: u32, y: u32, a: u8| {
        if x < W && y < H {
            let i = ((y * W + x) * 4) as usize;
            buf[i + 3] = buf[i + 3].max(a);
        }
    };
    // Body 4..36 x 13..31, stroke 3px, rounded-ish corners.
    let (x0, y0, x1, y1) = (3u32, 12u32, 37u32, 32u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let edge = x < x0 + 3 || x >= x1 - 3 || y < y0 + 3 || y >= y1 - 3;
            let corner = (x == x0 || x == x1 - 1) && (y == y0 || y == y1 - 1);
            if edge && !corner {
                put(x, y, 255);
            }
        }
    }
    // Terminal.
    for y in 18..26 {
        for x in 38..41 {
            put(x, y, 255);
        }
    }
    // Fill.
    if let Some(level) = fill {
        let inner_w = (x1 - 3 - 2) - (x0 + 3 + 2);
        let w = ((inner_w as f32) * (level.min(100) as f32) / 100.0).round() as u32;
        let w = if level > 0 { w.max(2) } else { 0 };
        for y in (y0 + 5)..(y1 - 5) {
            for x in (x0 + 5)..(x0 + 5 + w) {
                put(x, y, 255);
            }
        }
    }
    Image::new_owned(buf, W, H)
}

pub fn tooltip(s: &Snapshot, lang: &str) -> String {
    let t = i18n::get(lang);
    let name = if s.model.is_empty() { "EcoFlow".to_string() } else { s.model.clone() };
    match s.soc {
        None => format!("{name} – {}", t.no_data),
        Some(soc) => {
            let mut out = format!("{name} – {soc}%");
            if s.charging == Some(true) {
                out.push_str(&format!(", {}", t.charging));
            }
            if let (Some(i), Some(o)) = (s.watts_in, s.watts_out) {
                out.push_str(&format!("\n↓ {i} {w}  ↑ {o} {w}", w = t.watts));
            }
            out
        }
    }
}

/// Place the popup next to the tray icon, inside the monitor that holds it.
pub fn place_popup<R: Runtime>(win: &WebviewWindow<R>, tray_rect: Option<Rect>) {
    let Ok(size) = win.outer_size() else { return };
    let scale = win.scale_factor().unwrap_or(1.0);
    let margin = (8.0 * scale) as i32;

    let (icon_pos, icon_size) = match tray_rect {
        Some(r) => (r.position.to_physical::<f64>(scale), r.size.to_physical::<f64>(scale)),
        None => {
            // No tray coordinates (e.g. opened from the second instance): bottom-right.
            if let Ok(Some(m)) = win.primary_monitor() {
                let p = m.position();
                let s = m.size();
                let _ = win.set_position(PhysicalPosition::new(
                    p.x + s.width as i32 - size.width as i32 - margin,
                    p.y + s.height as i32 - size.height as i32 - margin * 7,
                ));
            }
            return;
        }
    };

    let cx = icon_pos.x + icon_size.width / 2.0;
    let cy = icon_pos.y + icon_size.height / 2.0;
    let monitor = win
        .available_monitors()
        .ok()
        .and_then(|ms| {
            ms.into_iter().find(|m| {
                let p = m.position();
                let s = m.size();
                cx >= p.x as f64
                    && cx < (p.x + s.width as i32) as f64
                    && cy >= p.y as f64
                    && cy < (p.y + s.height as i32) as f64
            })
        })
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };
    let mp = m.position();
    let ms = m.size();
    let (mx0, my0) = (mp.x, mp.y);
    let (mx1, my1) = (mp.x + ms.width as i32, mp.y + ms.height as i32);

    let w = size.width as i32;
    let h = size.height as i32;
    let mut x = cx as i32 - w / 2;
    let at_top = cy < (my0 + my1) as f64 / 2.0;
    let mut y = if at_top {
        (icon_pos.y + icon_size.height) as i32 + margin / 2
    } else {
        icon_pos.y as i32 - h - margin / 2
    };
    // Vertical taskbar (left/right): put the popup beside the icon.
    let at_side = icon_size.height > 0.0 && (cx as i32 - mx0 < 120 || mx1 - (cx as i32) < 120) && !at_top && (cy as i32) < my1 - 120;
    if at_side {
        x = if cx as i32 - mx0 < 120 { (icon_pos.x + icon_size.width) as i32 + margin } else { icon_pos.x as i32 - w - margin };
        y = cy as i32 - h / 2;
    }
    x = x.clamp(mx0 + margin, mx1 - w - margin);
    y = y.clamp(my0 + margin, my1 - h - margin);
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

pub fn popup<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("popup")
}
