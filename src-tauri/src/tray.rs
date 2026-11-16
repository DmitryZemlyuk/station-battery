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

/// macOS: a template (alpha-only) battery in the system's proportions; the number goes
/// into the menu bar title. 27×18 pt canvas rendered at 3x, so it is sharp on any display.
pub fn render_battery_template(fill: Option<u8>, charging: bool) -> Image<'static> {
    let level = fill.map(|v| v.min(100) as f32);
    let w = level.map(|v| if v > 0.0 { (17.0 * v / 100.0).max(2.0) } else { 0.0 }).unwrap_or(0.0);
    let body = r#"<rect x="1.5" y="3.5" width="22" height="11" rx="3.4" fill="none" stroke="black" stroke-opacity="0.55" stroke-width="1"/>
<path d="M25 7.2v3.6c.9-.3 1.5-1 1.5-1.8s-.6-1.5-1.5-1.8z" fill="black" fill-opacity="0.55"/>"#;
    let svg = if charging {
        // Bolt on a 16-unit grid, moved to the centre of the 22×11 body.
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 27 18" width="27" height="18">
<defs><mask id="m"><rect width="27" height="18" fill="white"/><path d="{BOLT}" fill="black" stroke="black" stroke-width="2.2" stroke-linejoin="round" transform="translate(5.1 1)"/></mask></defs>
<g mask="url(#m)">{body}<rect x="4" y="6" width="{w}" height="6" rx="1.6" fill="black"/></g>
<path d="{BOLT}" fill="black" transform="translate(5.1 1)"/>
</svg>"#
        )
    } else {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 27 18" width="27" height="18">
{body}<rect x="4" y="6" width="{w}" height="6" rx="1.6" fill="black"/>
</svg>"#
        )
    };
    rasterize(&svg, 81, 54)
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub x: i32,
    /// Bottom edge when `bottom` is true, otherwise the top edge.
    pub edge_y: i32,
    pub bottom: bool,
    /// Vertical bounds of the monitor (already shrunk by the margin).
    pub min_y: i32,
    pub max_y: i32,
}

impl Anchor {
    /// Top-left y for a popup of the given physical height.
    pub fn y_for(&self, height: i32) -> i32 {
        let y = if self.bottom { self.edge_y - height } else { self.edge_y };
        let hi = (self.max_y - height).max(self.min_y);
        y.clamp(self.min_y, hi)
    }
}

/// Decide where the popup goes. Pure geometry (physical pixels), unit-tested.
/// `icon` is (x, y, w, h) of the tray icon; `mon` is (x0, y0, x1, y1).
pub fn compute_anchor(icon: Option<(f64, f64, f64, f64)>, mon: (i32, i32, i32, i32), w: i32, margin: i32) -> Anchor {
    let (mx0, my0, mx1, my1) = mon;
    let min_y = my0 + margin;
    let max_y = my1 - margin;
    let clamp_x = |x: i32| x.clamp(mx0 + margin, (mx1 - w - margin).max(mx0 + margin));
    let Some((ix, iy, iw, ih)) = icon else {
        // No tray coordinates (opened from the menu or a second launch): bottom-right,
        // above a typical taskbar.
        return Anchor { x: clamp_x(mx1 - w - margin), edge_y: my1 - margin * 7, bottom: true, min_y, max_y };
    };
    let cx = (ix + iw / 2.0) as i32;
    let cy = (iy + ih / 2.0) as i32;
    let at_top = cy < (my0 + my1) / 2;
    // Vertical taskbar on the left/right edge, icon not in the bottom corner.
    let near_left = cx - mx0 < 120;
    let near_right = mx1 - cx < 120;
    if !at_top && (near_left || near_right) && cy < my1 - 120 {
        let x = if near_left { (ix + iw) as i32 + margin } else { ix as i32 - w - margin };
        return Anchor { x: clamp_x(x), edge_y: cy + 60, bottom: true, min_y, max_y };
    }
    if at_top {
        Anchor { x: clamp_x(cx - w / 2), edge_y: (iy + ih) as i32 + margin / 2, bottom: false, min_y, max_y }
    } else {
        Anchor { x: clamp_x(cx - w / 2), edge_y: iy as i32 - margin / 2, bottom: true, min_y, max_y }
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
    let mon = (mp.x, mp.y, mp.x + ms.width as i32, mp.y + ms.height as i32);
    // The popup takes the scale of the monitor it is shown on.
    let scale = monitor.scale_factor();
    let w = (popup_width() * scale).round() as i32;
    let h = (height * scale).round() as i32;
    let margin = (8.0 * scale).round() as i32;

    let anchor = compute_anchor(icon, mon, w, margin);
    let _ = win.set_position(PhysicalPosition::new(anchor.x, anchor.y_for(h)));
    Some(anchor)
}

pub fn popup<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("popup")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1080);

    #[test]
    fn bottom_taskbar_keeps_bottom_edge() {
        // Tray icon in a bottom taskbar (48 px high).
        let a = compute_anchor(Some((1700.0, 1040.0, 24.0, 24.0)), MON, 300, 8);
        assert!(a.bottom);
        assert_eq!(a.edge_y, 1036);
        // Growing or shrinking never moves the bottom edge, however often it is applied.
        assert_eq!(a.y_for(140) + 140, 1036);
        assert_eq!(a.y_for(500) + 500, 1036);
        assert_eq!(a.y_for(500), a.y_for(500));
        assert_eq!(a.x, 1712 - 150);
    }

    #[test]
    fn top_menu_bar_hangs_below_icon() {
        let a = compute_anchor(Some((1500.0, 0.0, 30.0, 24.0)), MON, 300, 8);
        assert!(!a.bottom);
        assert_eq!(a.y_for(140), 28);
        assert_eq!(a.y_for(500), 28);
    }

    #[test]
    fn stays_on_screen() {
        // Icon at the far right: popup is clamped inside the monitor.
        let a = compute_anchor(Some((1900.0, 1050.0, 16.0, 16.0)), MON, 300, 8);
        assert_eq!(a.x, 1920 - 300 - 8);
        // A popup taller than the space above the taskbar is clamped to the top margin.
        assert_eq!(a.y_for(2000), 8);
        // Fallback without tray coordinates.
        let f = compute_anchor(None, MON, 300, 8);
        assert!(f.bottom);
        assert_eq!(f.y_for(200) + 200, 1080 - 56);
    }

    #[test]
    fn second_monitor_offsets() {
        let mon = (1920, -200, 1920 + 2560, 1240);
        let a = compute_anchor(Some((4300.0, 1200.0, 24.0, 24.0)), mon, 300, 8);
        assert!(a.bottom);
        assert!(a.x >= 1920 + 8 && a.x + 300 <= 1920 + 2560 - 8);
        assert_eq!(a.y_for(140) + 140, 1196);
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
        let t = render_battery_template(Some(50), true);
        assert_eq!((t.width(), t.height()), (81, 54));
        assert!(t.rgba().chunks(4).any(|p| p[3] > 200));
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
        for (i, (fill, ch)) in [(Some(78u8), false), (Some(78), true), (Some(14), false), (Some(100), true), (None, false)].iter().enumerate() {
            let t = render_battery_template(*fill, *ch);
            std::fs::write(format!("{dir}/mac-{i}-{}x{}.rgba", t.width(), t.height()), t.rgba()).unwrap();
        }
    }
}
