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
    pub charging: bool,
    pub light_bar: bool,
    /// Icon size in physical pixels. The icon is drawn at exactly the size the tray
    /// shows it, because Windows scales a mismatched icon down into a blur.
    pub px: u32,
}

/// Tray icon size for a display scale factor (Windows small icon: 16 px at 100%).
pub fn tray_px(scale: f64) -> u32 {
    if cfg!(windows) {
        ((16.0 * scale).round() as u32).clamp(16, 64)
    } else {
        24
    }
}

pub fn look(s: &Snapshot, low_threshold: u8, px: u32) -> TrayLook {
    let light_bar = system_uses_light_theme();
    let charging = s.charging == Some(true);
    match s.soc {
        Some(soc) if !s.stale => TrayLook {
            text: soc.to_string(),
            tone: if charging {
                Tone::Charging
            } else if soc <= low_threshold {
                Tone::Low
            } else {
                Tone::Normal
            },
            fill: Some(soc),
            charging,
            light_bar,
            px,
        },
        Some(soc) => TrayLook { text: soc.to_string(), tone: Tone::Dim, fill: Some(soc), charging: false, light_bar, px },
        None => TrayLook { text: "–".into(), tone: Tone::Dim, fill: None, charging: false, light_bar, px },
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

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Rasterizes an SVG (anti-aliased) to straight-alpha RGBA at exactly `w`×`h` pixels.
fn rasterize(svg: &str, w: u32, h: u32) -> Image<'static> {
    use resvg::{tiny_skia, usvg};
    let empty = || Image::new_owned(vec![0u8; (w * h * 4) as usize], w, h);
    let Ok(tree) = usvg::Tree::from_str(svg, &usvg::Options::default()) else { return empty() };
    let Some(mut pixmap) = tiny_skia::Pixmap::new(w, h) else { return empty() };
    let size = tree.size();
    let t = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, t, &mut pixmap.as_mut());
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Image::new_owned(buf, w, h)
}

/// Windows / Linux: the number itself is the icon, drawn at the tray's pixel size.
pub fn render_number(l: &TrayLook) -> Image<'static> {
    let s = l.px.max(8);
    let sf32 = s as f32;
    let mut buf = vec![0u8; (s * s * 4) as usize];
    let font = FontRef::try_from_slice(FONT).expect("font");
    let [r, g, b] = color(l);

    // The biggest size that fits the width (the font is condensed bold, so two digits
    // still get most of the height).
    let width_at = |px: f32| {
        let sf = font.as_scaled(PxScale::from(px));
        l.text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum::<f32>()
    };
    let mut px = sf32 * 1.02;
    while px > 6.0 && width_at(px) > sf32 {
        px -= 0.25;
    }
    let sf = font.as_scaled(PxScale::from(px));
    let total_w = width_at(px);
    // Digits have no descenders: centre the cap height, on a whole pixel so the
    // horizontal strokes stay sharp.
    let cap = px * 0.73;
    let baseline = ((sf32 + cap) / 2.0).round();
    let mut x = (sf32 - total_w) / 2.0;

    for ch in l.text.chars() {
        let id = sf.glyph_id(ch);
        let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
        x += sf.h_advance(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let px_x = bb.min.x as i32 + gx as i32;
                let px_y = bb.min.y as i32 + gy as i32;
                if px_x < 0 || px_y < 0 || px_x >= s as i32 || px_y >= s as i32 {
                    return;
                }
                let i = ((px_y as u32 * s + px_x as u32) * 4) as usize;
                // A slight gamma lift keeps thin strokes legible at 16 px.
                let a = (cov.clamp(0.0, 1.0).powf(0.8) * 255.0) as u8;
                if a > buf[i + 3] {
                    buf[i] = r;
                    buf[i + 1] = g;
                    buf[i + 2] = b;
                    buf[i + 3] = a;
                }
            });
        }
    }
    Image::new_owned(buf, s, s)
}

/// Bolt used on the battery glyphs (16-unit grid, centred on the battery body).
const BOLT: &str = "M8.6 3.2 5.2 8.4h2.1l-.9 4.4 3.9-5.6H8.1l1.3-4z";

/// Windows / Linux alternative: a battery glyph in the Fluent shape, filled to the
/// level. Drawn on a 16-unit grid so it is pixel-exact at 100% scale.
pub fn render_battery_color(l: &TrayLook) -> Image<'static> {
    let outline = hex(if l.light_bar { [0x1A, 0x1A, 0x1A] } else { [0xFF, 0xFF, 0xFF] });
    let fill = match l.tone {
        Tone::Normal | Tone::Dim => outline.clone(),
        _ => hex(color(l)),
    };
    let opacity = if l.tone == Tone::Dim { 0.55 } else { 1.0 };
    let level = l.fill.map(|v| v.min(100) as f32);
    let w = level.map(|v| if v > 0.0 { (10.0 * v / 100.0).max(1.0) } else { 0.0 }).unwrap_or(0.0);
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">
<rect x="0.5" y="4.5" width="13" height="7" rx="2" fill="none" stroke="{outline}" stroke-width="1"/>
<path d="M14.5 6.5v3" stroke="{outline}" stroke-width="1" stroke-linecap="round" fill="none"/>
<rect x="2" y="6" width="{w}" height="4" rx="0.8" fill="{fill}" fill-opacity="{opacity}"/>"#
    );
    if l.charging {
        // Knock the bolt's surroundings out of the fill, then draw it in the outline colour.
        svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">
<defs><mask id="m"><rect width="16" height="16" fill="white"/><path d="{BOLT}" fill="black" stroke="black" stroke-width="2" stroke-linejoin="round" transform="translate(0.1 0)"/></mask></defs>
<g mask="url(#m)">
<rect x="0.5" y="4.5" width="13" height="7" rx="2" fill="none" stroke="{outline}" stroke-width="1"/>
<rect x="2" y="6" width="{w}" height="4" rx="0.8" fill="{fill}"/>
</g>
<path d="M14.5 6.5v3" stroke="{outline}" stroke-width="1" stroke-linecap="round" fill="none"/>
<path d="{BOLT}" fill="{outline}" transform="translate(0.1 0)"/>"#
        );
    }
    svg.push_str("</svg>");
    rasterize(&svg, l.px.max(8), l.px.max(8))
}

/// macOS: a template (alpha-only) battery with the percentage inside it, like the
/// system battery item. The level is the solid part, the rest is dimmed, and the digits
/// (plus a bolt while charging) are knocked out. Drawn at 3x of an 18 pt tall canvas;
/// the width grows with the text.
pub fn render_battery_template(text: Option<&str>, fill: Option<u8>, charging: bool) -> Image<'static> {
    const H: u32 = 54;
    const PX: f32 = 30.0; // digit size: 10 pt
    let font = FontRef::try_from_slice(FONT).expect("font");
    let sf = font.as_scaled(PxScale::from(PX));
    let text = text.filter(|_| fill.is_some());
    let text_w: f32 = text.map(|t| t.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()).unwrap_or(0.0);
    let bolt_w = if charging && text.is_some() { 13.0 } else { 0.0 };
    let gap = if bolt_w > 0.0 { 2.0 } else { 0.0 };
    let content = text_w + gap + bolt_w;

    let (bx, by, bh) = (3.0f32, 8.5f32, 37.0f32);
    let body_w = (content + 20.0).max(69.0).ceil();
    let w = (bx + body_w + 9.0).ceil() as u32;
    let cap_x = bx + body_w + 3.0;
    let svg = match fill {
        None => format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {H}" width="{w}" height="{H}">
<rect x="{x}" y="{y}" width="{bw}" height="{h}" rx="9.5" fill="none" stroke="black" stroke-opacity="0.6" stroke-width="3"/>
<rect x="{cap_x}" y="20.5" width="4.5" height="13" rx="2.2" fill="black" fill-opacity="0.6"/>
</svg>"#,
            x = bx + 1.5,
            y = by + 1.5,
            bw = body_w - 3.0,
            h = bh - 3.0
        ),
        Some(level) => {
            let level = level.min(100) as f32 / 100.0;
            let cap_opacity = if level >= 1.0 { 1.0 } else { 0.42 };
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {H}" width="{w}" height="{H}">
<defs><clipPath id="c"><rect x="{bx}" y="{by}" width="{body_w}" height="{bh}" rx="11"/></clipPath></defs>
<rect x="{bx}" y="{by}" width="{body_w}" height="{bh}" rx="11" fill="black" fill-opacity="0.42"/>
<rect x="{bx}" y="{by}" width="{lw}" height="{bh}" fill="black" clip-path="url(#c)"/>
<rect x="{cap_x}" y="20.5" width="4.5" height="13" rx="2.2" fill="black" fill-opacity="{cap_opacity}"/>
</svg>"#,
                lw = body_w * level
            )
        }
    };
    let base = rasterize(&svg, w, H);
    let Some(text) = text else { return base };
    let mut buf = base.rgba().to_vec();

    // Coverage of everything that is knocked out of the battery.
    let mut punch = vec![0f32; (w * H) as usize];
    let mut x = bx + (body_w - content) / 2.0;
    let baseline = (by + (bh + PX * 0.73) / 2.0).round();
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        let glyph = id.with_scale_and_position(PxScale::from(PX), ab_glyph::point(x, baseline));
        x += sf.h_advance(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px_x, px_y) = (bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32);
                if px_x >= 0 && px_y >= 0 && (px_x as u32) < w && (px_y as u32) < H {
                    let i = (px_y as u32 * w + px_x as u32) as usize;
                    punch[i] = punch[i].max(cov.clamp(0.0, 1.0));
                }
            });
        }
    }
    if bolt_w > 0.0 {
        // BOLT spans x 5.2..10.3, y 3.2..12.8 on its 16-unit grid; 2.5x makes it 24 px tall.
        let (tx, ty) = (x + gap - 5.2 * 2.5, 15.0 - 3.2 * 2.5);
        let bolt = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {H}" width="{w}" height="{H}"><path d="{BOLT}" fill="black" transform="translate({tx} {ty}) scale(2.5)"/></svg>"#
        );
        for (i, p) in rasterize(&bolt, w, H).rgba().chunks(4).enumerate() {
            punch[i] = punch[i].max(p[3] as f32 / 255.0);
        }
    }
    for (i, cov) in punch.iter().enumerate() {
        if *cov > 0.0 {
            let a = &mut buf[i * 4 + 3];
            *a = (*a as f32 * (1.0 - cov)).round() as u8;
        }
    }
    Image::new_owned(buf, w, H)
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

/// Where the popup is pinned, so later height changes can be applied idempotently.
/// All values describe the *visible* popup in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub x: i32,
    /// Bottom edge when `bottom` is true, otherwise the top edge.
    pub edge_y: i32,
    pub bottom: bool,
    /// Vertical bounds the popup must stay within (work area minus the gap).
    pub min_y: i32,
    pub max_y: i32,
    /// Offset of the visible area inside the OS window. On Windows 11 an undecorated
    /// window with a shadow keeps an invisible frame (about 8 px left/right/bottom).
    pub frame_dx: i32,
    pub frame_dy: i32,
}

impl Anchor {
    /// Top-left y of the visible popup for the given physical height.
    pub fn y_for(&self, height: i32) -> i32 {
        let y = if self.bottom { self.edge_y - height } else { self.edge_y };
        let hi = (self.max_y - height).max(self.min_y);
        y.clamp(self.min_y, hi)
    }

    /// Position to give the OS window so that the visible popup lands on the anchor.
    pub fn window_pos(&self, height: i32) -> (i32, i32) {
        (self.x - self.frame_dx, self.y_for(height) - self.frame_dy)
    }
}

/// Inputs for [`compute_anchor`], all in physical pixels. Rects are (x0, y0, x1, y1).
#[derive(Clone, Copy, Debug)]
pub struct Screen {
    pub monitor: (i32, i32, i32, i32),
    /// The monitor minus taskbar / menu bar / Dock.
    pub work: (i32, i32, i32, i32),
    /// Popup width.
    pub w: i32,
    /// Distance kept from the bar and the screen edges.
    pub gap: i32,
    /// Where to go when there is no tray icon position: under the top bar (macOS)
    /// instead of above the bottom one.
    pub prefer_top: bool,
}

/// Decide where the popup goes. Pure geometry, unit-tested.
/// `icon` is (x, y, w, h) of the tray icon.
pub fn compute_anchor(icon: Option<(f64, f64, f64, f64)>, sc: Screen) -> Anchor {
    let (mx0, my0, mx1, my1) = sc.monitor;
    // A missing or nonsensical work area falls back to the whole monitor.
    let (wx0, wy0, wx1, wy1) = {
        let (a, b, c, d) = sc.work;
        if c - a >= sc.w && d - b >= 100 && a >= mx0 && b >= my0 && c <= mx1 && d <= my1 {
            sc.work
        } else {
            sc.monitor
        }
    };
    let (w, gap) = (sc.w, sc.gap);
    // Under a top bar the popup hangs closer (half the gap), as system popovers do.
    let min_y = wy0 + gap / 2;
    let max_y = wy1 - gap;
    let clamp_x = |x: i32| x.clamp(wx0 + gap, (wx1 - w - gap).max(wx0 + gap));
    let anchor = |x: i32, edge_y: i32, bottom: bool| Anchor { x: clamp_x(x), edge_y, bottom, min_y, max_y, frame_dx: 0, frame_dy: 0 };

    let Some((ix, iy, iw, ih)) = icon else {
        // Opened from the menu or by a second launch: the corner next to the tray.
        let x = wx1 - w - gap;
        return if sc.prefer_top { anchor(x, wy0 + gap / 2, false) } else { anchor(x, wy1 - gap, true) };
    };
    let cx = (ix + iw / 2.0) as i32;
    let cy = (iy + ih / 2.0) as i32;
    // Which bar holds the icon? Exact when the work area shows it (the icon lies inside
    // the strip the bar takes from the monitor)...
    #[derive(PartialEq)]
    enum Bar {
        Bottom,
        Top,
        Left,
        Right,
    }
    let bar = if cy >= wy1 && wy1 < my1 {
        Bar::Bottom
    } else if cy < wy0 && wy0 > my0 {
        Bar::Top
    } else if cx < wx0 && wx0 > mx0 {
        Bar::Left
    } else if cx >= wx1 && wx1 < mx1 {
        Bar::Right
    } else {
        // ...otherwise (auto-hidden bar) guess from the nearest monitor edge. A side bar
        // is only assumed when the icon hugs the side and is clearly away from the
        // top and bottom, since tray icons of a horizontal bar also sit near a corner.
        let d_v = (cy - my0).min(my1 - cy);
        let d_h = (cx - mx0).min(mx1 - cx);
        if d_h < d_v && d_v > 100 {
            if cx - mx0 < mx1 - cx {
                Bar::Left
            } else {
                Bar::Right
            }
        } else if cy - my0 < my1 - cy {
            Bar::Top
        } else {
            Bar::Bottom
        }
    };
    match bar {
        // Above a bottom taskbar (or above the icon when the taskbar auto-hides).
        Bar::Bottom => anchor(cx - w / 2, (wy1 - gap).min(iy as i32 - gap), true),
        // Under the menu bar.
        Bar::Top => anchor(cx - w / 2, (wy0 + gap / 2).max((iy + ih) as i32 + gap / 2), false),
        Bar::Left => anchor((wx0 + gap).max((ix + iw) as i32 + gap), (cy + 60).min(wy1 - gap), true),
        Bar::Right => anchor((wx1 - w - gap).min(ix as i32 - w - gap), (cy + 60).min(wy1 - gap), true),
    }
}

/// Popup width in logical pixels (matches the card width in ui/popup.css).
pub fn popup_width() -> f64 {
    // Linux (development only) uses the Windows look.
    if cfg!(target_os = "macos") {
        300.0
    } else {
        320.0
    }
}

/// Offset of the client area inside the window frame (see [`Anchor::frame_dx`]).
fn frame_offset<R: Runtime>(win: &WebviewWindow<R>, scale: f64) -> (i32, i32) {
    let (Ok(inner), Ok(outer)) = (win.inner_position(), win.outer_position()) else { return (0, 0) };
    let (dx, dy) = (inner.x - outer.x, inner.y - outer.y);
    let max = (40.0 * scale) as i32;
    // Anything else is a platform reporting nonsense for a hidden window.
    if (0..=max).contains(&dx) && (0..=max).contains(&dy) {
        (dx, dy)
    } else {
        (0, 0)
    }
}

/// Place the popup next to the tray icon, inside the monitor that holds it.
/// `height` is the popup's current logical height. Sizes are computed from known
/// values instead of asking the OS, which may report nonsense for a hidden window.
pub fn place_popup<R: Runtime>(win: &WebviewWindow<R>, tray_rect: Option<Rect>, height: f64) -> Option<Anchor> {
    let win_scale = win.scale_factor().unwrap_or(1.0);
    let icon = tray_rect.map(|r| {
        let p = r.position.to_physical::<f64>(win_scale);
        let s = r.size.to_physical::<f64>(win_scale);
        (p.x, p.y, s.width, s.height)
    });
    let monitor = icon
        .and_then(|(ix, iy, iw, ih)| {
            let (cx, cy) = (ix + iw / 2.0, iy + ih / 2.0);
            win.available_monitors().ok()?.into_iter().find(|m| {
                let p = m.position();
                let s = m.size();
                cx >= p.x as f64
                    && cx < (p.x + s.width as i32) as f64
                    && cy >= p.y as f64
                    && cy < (p.y + s.height as i32) as f64
            })
        })
        .or_else(|| win.primary_monitor().ok().flatten())?;
    let mp = monitor.position();
    let ms = monitor.size();
    let wa = monitor.work_area();
    // The popup takes the scale of the monitor it is shown on.
    let scale = monitor.scale_factor();
    let mac = cfg!(target_os = "macos");
    let screen = Screen {
        monitor: (mp.x, mp.y, mp.x + ms.width as i32, mp.y + ms.height as i32),
        work: (
            wa.position.x,
            wa.position.y,
            wa.position.x + wa.size.width as i32,
            wa.position.y + wa.size.height as i32,
        ),
        w: (popup_width() * scale).round() as i32,
        // Windows flyouts keep 12 px from the taskbar and the screen edge.
        gap: ((if mac { 8.0 } else { 12.0 }) * scale).round() as i32,
        prefer_top: mac,
    };
    let h = (height * scale).round() as i32;

    let mut anchor = compute_anchor(icon, screen);
    (anchor.frame_dx, anchor.frame_dy) = frame_offset(win, scale);
    let (x, y) = anchor.window_pos(h);
    let _ = win.set_position(PhysicalPosition::new(x, y));
    Some(anchor)
}

pub fn popup<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("popup")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1920×1080 with a 48 px taskbar at the bottom.
    const WIN: Screen = Screen { monitor: (0, 0, 1920, 1080), work: (0, 0, 1920, 1032), w: 320, gap: 12, prefer_top: false };
    // 1512×982 MacBook: 25 px menu bar, 70 px Dock.
    const MAC: Screen = Screen { monitor: (0, 0, 1512, 982), work: (0, 25, 1512, 912), w: 300, gap: 8, prefer_top: true };

    #[test]
    fn windows_flyout_sits_above_the_taskbar() {
        let a = compute_anchor(Some((1700.0, 1044.0, 24.0, 24.0)), WIN);
        assert!(a.bottom);
        // 12 px above the taskbar, never over it, whatever the height.
        assert_eq!(a.edge_y, 1032 - 12);
        assert_eq!(a.y_for(140) + 140, 1020);
        assert_eq!(a.y_for(500) + 500, 1020);
        // Centred on the icon.
        assert_eq!(a.x, 1712 - 160);
        // Applying it repeatedly gives the same place (no drift).
        assert_eq!(a.window_pos(300), a.window_pos(300));
    }

    #[test]
    fn windows_flyout_keeps_the_edge_margin_and_frame_offset() {
        // Icon near the right edge: the visible popup stops 12 px from the screen edge.
        let mut a = compute_anchor(Some((1890.0, 1044.0, 24.0, 24.0)), WIN);
        assert_eq!(a.x + 320, 1920 - 12);
        // The invisible frame of a shadowed window moves the OS window, not the popup.
        (a.frame_dx, a.frame_dy) = (8, 1);
        let (wx, wy) = a.window_pos(200);
        assert_eq!((wx + 8, wy + 1), (a.x, a.y_for(200)));
        assert_eq!(wx + 8 + 320, 1920 - 12);
    }

    #[test]
    fn auto_hidden_taskbar_uses_the_icon() {
        let sc = Screen { work: (0, 0, 1920, 1080), ..WIN };
        let a = compute_anchor(Some((1700.0, 1044.0, 24.0, 24.0)), sc);
        assert!(a.bottom);
        assert_eq!(a.edge_y, 1044 - 12);
    }

    #[test]
    fn vertical_taskbar_puts_the_flyout_beside_it() {
        // Taskbar on the right (Windows 10), tray at its bottom.
        let sc = Screen { work: (0, 0, 1858, 1080), ..WIN };
        let a = compute_anchor(Some((1880.0, 1000.0, 24.0, 24.0)), sc);
        assert_eq!(a.x + 320, 1858 - 12);
        assert!(a.y_for(300) + 300 <= 1080 - 12);
        // And on the left.
        let sc = Screen { work: (62, 0, 1920, 1080), ..WIN };
        let a = compute_anchor(Some((20.0, 1000.0, 24.0, 24.0)), sc);
        assert_eq!(a.x, 62 + 12);
        // Auto-hidden side taskbar, icon half-way up: still beside the icon.
        let sc = Screen { work: (0, 0, 1920, 1080), ..WIN };
        let a = compute_anchor(Some((1880.0, 500.0, 24.0, 24.0)), sc);
        assert_eq!(a.x + 320, 1880 - 12);
        // Auto-hidden bottom taskbar, icon in the very corner: still above it.
        let a = compute_anchor(Some((1890.0, 1044.0, 24.0, 24.0)), sc);
        assert!(a.bottom);
        assert_eq!((a.x + 320, a.edge_y), (1920 - 12, 1044 - 12));
    }

    #[test]
    fn mac_panel_hangs_under_the_menu_bar_despite_the_dock() {
        let a = compute_anchor(Some((1200.0, 2.0, 30.0, 22.0)), MAC);
        assert!(!a.bottom);
        assert_eq!(a.y_for(140), 29);
        assert_eq!(a.y_for(400), 29);
        assert_eq!(a.x, 1215 - 150);
        // Near the right edge it stays on screen.
        let a = compute_anchor(Some((1480.0, 2.0, 30.0, 22.0)), MAC);
        assert_eq!(a.x + 300, 1512 - 8);
    }

    #[test]
    fn without_an_icon_it_goes_to_the_tray_corner() {
        let a = compute_anchor(None, WIN);
        assert!(a.bottom);
        assert_eq!((a.x + 320, a.y_for(200) + 200), (1920 - 12, 1032 - 12));
        let a = compute_anchor(None, MAC);
        assert!(!a.bottom);
        assert_eq!((a.x + 300, a.y_for(200)), (1512 - 8, 29));
    }

    #[test]
    fn stays_on_screen_and_handles_bad_work_areas() {
        // Taller than the work area: clamped to the top margin.
        let a = compute_anchor(Some((1700.0, 1044.0, 24.0, 24.0)), WIN);
        assert_eq!(a.y_for(5000), 6);
        // Second monitor to the right, above the primary's origin.
        let sc = Screen { monitor: (1920, -200, 4480, 1240), work: (1920, -200, 4480, 1192), ..WIN };
        let a = compute_anchor(Some((4300.0, 1200.0, 24.0, 24.0)), sc);
        assert!(a.x >= 1920 + 12 && a.x + 320 <= 4480 - 12);
        assert_eq!(a.y_for(140) + 140, 1192 - 12);
        // A zero work area (some drivers / hidden windows) falls back to the monitor.
        let sc = Screen { work: (0, 0, 0, 0), ..WIN };
        let a = compute_anchor(Some((1700.0, 1044.0, 24.0, 24.0)), sc);
        assert_eq!(a.edge_y, 1044 - 12);
    }

    #[test]
    fn icons_have_the_requested_size_and_content() {
        for px in [16u32, 20, 24, 32] {
            let l = TrayLook { text: "78".into(), tone: Tone::Normal, fill: Some(78), charging: false, light_bar: false, px };
            for img in [render_number(&l), render_battery_color(&l)] {
                assert_eq!((img.width(), img.height()), (px, px));
                assert!(img.rgba().chunks(4).filter(|p| p[3] > 128).count() > (px * px / 12) as usize);
            }
        }
        // macOS: 18 pt tall at 3x; wider for three digits plus the bolt; digits are holes.
        let t = render_battery_template(Some("50"), Some(50), false);
        assert_eq!(t.height(), 54);
        let wide = render_battery_template(Some("100"), Some(100), true);
        assert!(wide.width() > t.width());
        let solid = |img: &Image| img.rgba().chunks(4).filter(|p| p[3] > 200).count();
        assert!(solid(&render_battery_template(None, Some(100), false)) > solid(&render_battery_template(Some("100"), Some(100), false)));
        // No data: an empty outline, no digits.
        let empty = render_battery_template(Some("–"), None, false);
        assert_eq!(empty.width(), t.width());
        assert_eq!(solid(&empty), 0);
        assert_eq!(tray_px(1.0), if cfg!(windows) { 16 } else { 24 });
    }

    /// Dumps the tray icons for a visual check: SB_ICON_DIR=/tmp/icons cargo test -- --ignored dump_icons
    #[test]
    #[ignore]
    fn dump_icons() {
        let Ok(dir) = std::env::var("SB_ICON_DIR") else { return };
        let mut n = 0;
        for px in [16u32, 20, 24, 32] {
            for light_bar in [false, true] {
                for (text, tone, fill, charging) in [
                    ("78", Tone::Normal, Some(78u8), false),
                    ("78", Tone::Charging, Some(78), true),
                    ("14", Tone::Low, Some(14), false),
                    ("100", Tone::Charging, Some(100), true),
                    ("5", Tone::Charging, Some(5), true),
                    ("–", Tone::Dim, None, false),
                ] {
                    let l = TrayLook { text: text.into(), tone, fill, charging, light_bar, px };
                    for (kind, img) in [("num", render_number(&l)), ("bat", render_battery_color(&l))] {
                        let theme = if light_bar { "light" } else { "dark" };
                        let path = format!("{dir}/{px:02}-{theme}-{n:03}-{kind}-{}x{}.rgba", img.width(), img.height());
                        std::fs::write(path, img.rgba()).unwrap();
                    }
                    n += 1;
                }
            }
        }
        for (i, (fill, ch)) in [(Some(78u8), false), (Some(78), true), (Some(14), false), (Some(100), false), (Some(100), true), (Some(5), true), (None, false)].iter().enumerate() {
            let text = fill.map(|v| v.to_string());
            let t = render_battery_template(text.as_deref(), *fill, *ch);
            std::fs::write(format!("{dir}/mac-{i}-{}x{}.rgba", t.width(), t.height()), t.rgba()).unwrap();
        }
    }
}
