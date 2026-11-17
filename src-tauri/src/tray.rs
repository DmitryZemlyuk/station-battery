//! Tray icon rendering and popup placement.

use crate::core::Snapshot;
use crate::i18n;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tauri::image::Image;
use tauri::{AppHandle, Manager, PhysicalPosition, Rect, Runtime, WebviewWindow};

/// Digits-only subsets of Inter (SIL OFL). Regular for the Windows tray, matching the
/// weight of the taskbar text; SemiBold for the macOS menu bar, where the digits are cut
/// out of the battery and need sturdier strokes.
static FONT: &[u8] = include_bytes!("../assets/digits.ttf");
static FONT_REGULAR: &[u8] = include_bytes!("../assets/digits-regular.ttf");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Normal,
    Charging,
    Low,
    Dim,
}

/// What the tray item shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Battery percentage.
    Percent,
    /// Grid voltage, in volts.
    Voltage,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TrayLook {
    pub kind: Kind,
    pub text: String,
    pub tone: Tone,
    pub fill: Option<u8>,
    pub charging: bool,
    pub light_bar: bool,
    /// Digit size setting ("s" | "m" | "l" | "xl").
    pub size: &'static str,
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

/// Grid voltage outside this range (230 V ±10%, rounded out) is shown as a problem.
const VOLTS_OK: std::ops::RangeInclusive<i64> = 200..=253;

/// `style` is the `tray_style` setting. "voltage" needs the cloud; without a reading
/// the item falls back to the percentage instead of going blank.
/// Normalises the setting to a static string (unknown values become "m").
pub fn digit_size(setting: &str) -> &'static str {
    match setting {
        "s" => "s",
        "l" => "l",
        "xl" => "xl",
        _ => "m",
    }
}

/// How much bigger than "m" (the system text size) each setting is.
pub fn size_factor(size: &str) -> f32 {
    match size {
        "s" => 8.0 / 9.0,
        "l" => 10.0 / 9.0,
        "xl" => 11.0 / 9.0,
        _ => 1.0,
    }
}

/// `lost` is the `tray_lost` setting: what to show when the station can't be reached.
/// "dash" (the default) shows "–"; "dim" keeps the last level, greyed out.
pub fn look(s: &Snapshot, low_threshold: u8, px: u32, style: &str, size: &str, lost: &str) -> TrayLook {
    let size = digit_size(size);
    let light_bar = system_uses_light_theme();
    let charging = s.charging == Some(true);
    if style == "voltage" {
        if let Some(v) = s.volts_in {
            return TrayLook {
                kind: Kind::Voltage,
                text: v.to_string(),
                tone: if VOLTS_OK.contains(&v) { Tone::Normal } else { Tone::Low },
                fill: s.soc,
                charging,
                light_bar,
                size,
                px,
            };
        }
    }
    match s.soc {
        Some(soc) if !s.stale => TrayLook {
            kind: Kind::Percent,
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
            size,
            px,
        },
        Some(soc) if lost == "dim" => TrayLook { kind: Kind::Percent, text: soc.to_string(), tone: Tone::Dim, fill: Some(soc), charging: false, light_bar, size, px },
        _ => TrayLook { kind: Kind::Percent, text: "–".into(), tone: Tone::Dim, fill: None, charging: false, light_bar, size, px },
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

/// Vertical extent of the digits at a font size, as (top, bottom) offsets from the
/// baseline in pixels (top is positive, bottom is the small overshoot below it).
/// Taken from the outline of "0" in font units, so there is no pixel rounding in it.
/// The font is a digits-only subset of Inter SemiBold (SIL OFL): the closest freely
/// embeddable match to Segoe UI on Windows and SF on macOS.
fn digit_extent(font: &FontRef, px: f32) -> (f32, f32) {
    let factor = px / font.height_unscaled();
    match font.outline(font.glyph_id('0')) {
        Some(o) => (o.bounds.max.y.max(o.bounds.min.y) * factor, o.bounds.max.y.min(o.bounds.min.y) * factor),
        None => (px * 0.6, 0.0),
    }
}

/// Baseline that centres the digits vertically in a box from `top` to `top + height`.
fn centred_baseline(font: &FontRef, px: f32, top: f32, height: f32) -> f32 {
    let (up, down) = digit_extent(font, px);
    (top + height / 2.0 + (up + down) / 2.0).round()
}

/// Font size (ab_glyph pixel scale) at which the digits are `h` pixels tall.
fn px_for_digit_height(font: &FontRef, h: f32) -> f32 {
    let (up, down) = digit_extent(font, 1.0);
    h / (up - down).max(0.1)
}

/// Pen advances for `text` at `px` with `tracking` pixels added between characters.
fn text_width(font: &FontRef, text: &str, px: f32, tracking: f32) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let n = text.chars().count();
    let adv: f32 = text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum();
    adv + tracking * n.saturating_sub(1) as f32
}

/// Where the ink of `text` starts relative to the pen start, and how wide it is: the
/// advance widths minus the empty side bearings of the first and last character.
fn ink_span(font: &FontRef, text: &str, px: f32, tracking: f32) -> (f32, f32) {
    let sf = font.as_scaled(PxScale::from(px));
    let k = px / font.height_unscaled();
    let (mut pen, mut left, mut right) = (0.0f32, f32::MAX, f32::MIN);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(o) = font.outline(id) {
            left = left.min(pen + o.bounds.min.x * k);
            right = right.max(pen + o.bounds.max.x * k);
        }
        pen += sf.h_advance(id) + tracking;
    }
    if left > right {
        (0.0, text_width(font, text, px, tracking))
    } else {
        (left, right - left)
    }
}

/// Size and letter spacing for `text` so the digits are `digit_h` tall and the line
/// fits in `max_w`. `spacing` is the letter spacing wanted (pixels between characters);
/// when the line is too wide it is given up first, then tightened a little below zero,
/// and only then does the size shrink.
fn fit_digits(font: &FontRef, text: &str, digit_h: f32, max_w: f32, spacing: f32) -> (f32, f32) {
    let mut px = px_for_digit_height(font, digit_h);
    let min_px = px * 0.55;
    let gaps = text.chars().count().saturating_sub(1) as f32;
    loop {
        // Down to 9% of the size per gap (about 1 px at tray sizes) of tighter spacing.
        let min_tracking = -0.09 * px;
        // The ink, not the advances: the font's own side bearings are empty space.
        let natural = ink_span(font, text, px, 0.0).1;
        if gaps == 0.0 {
            if natural <= max_w {
                return (px, 0.0);
            }
        } else {
            let room = (max_w - natural) / gaps;
            if room >= spacing {
                return (px, spacing);
            }
            if room >= min_tracking {
                return (px, room);
            }
        }
        if px <= min_px {
            return (px, min_tracking);
        }
        px -= 0.25;
    }
}

/// Letter spacing in the Windows tray: half a pixel at 100% (a touch looser than the
/// font's default, which looks cramped at 9 px), 1 px at 150% and 200%.
pub fn tray_spacing(px: u32) -> f32 {
    (px as f32 / 16.0).round().max(1.0) / 2.0
}

/// Draws `text` with its digits centred in `w`×`h` (or starting at `x0` when given),
/// calling `put(x, y, coverage)` for each touched pixel. Returns the pen end x.
fn draw_digits(
    font: &FontRef,
    text: &str,
    px: f32,
    tracking: f32,
    x0: f32,
    baseline: f32,
    (w, h): (u32, u32),
    mut put: impl FnMut(u32, u32, f32),
) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let mut x = x0;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
        x += sf.h_advance(id) + tracking;
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px_x, px_y) = (bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32);
                if px_x >= 0 && px_y >= 0 && (px_x as u32) < w && (px_y as u32) < h {
                    put(px_x as u32, px_y as u32, cov.clamp(0.0, 1.0));
                }
            });
        }
    }
    x - tracking
}

/// Digit height in the tray icon. "m" is the taskbar's own text (Segoe UI 12 px at
/// 100%, digits about 9 px); the other sizes are a pixel apart at 100%. Whole pixels,
/// so the strokes stay crisp.
pub fn tray_digit_height(px: u32, size: &str) -> f32 {
    (px as f32 * 0.5625 * size_factor(size)).round().max(6.0)
}

/// Windows / Linux: the number itself is the icon, drawn at the tray's pixel size.
/// Digits match the taskbar text size; three digits get tighter spacing and, only if
/// still too wide, a smaller size.
pub fn render_number(l: &TrayLook) -> Image<'static> {
    let s = l.px.max(8);
    let sf32 = s as f32;
    let mut buf = vec![0u8; (s * s * 4) as usize];
    let font = FontRef::try_from_slice(FONT_REGULAR).expect("font");
    let [r, g, b] = color(l);

    // One pixel of air on each side, like the system's own tray icons. Three digits
    // (100%, voltage) get the whole width instead: there it is worth more than the air.
    let target = tray_digit_height(s, l.size);
    let max_w = if l.text.chars().count() >= 3 { sf32 } else { sf32 - 2.0 };
    let (px, tracking) = fit_digits(&font, &l.text, target, max_w, tray_spacing(s));
    // Centre the ink (not the advances) and start it on a whole pixel.
    let (ink_left, ink_w) = ink_span(&font, &l.text, px, tracking);
    let baseline = centred_baseline(&font, px, 0.0, sf32);
    let x0 = ((sf32 - ink_w) / 2.0).round() - ink_left;
    draw_digits(&font, &l.text, px, tracking, x0, baseline, (s, s), |x, y, cov| {
        let i = ((y * s + x) * 4) as usize;
        let a = (cov.powf(0.85) * 255.0) as u8;
        if a > buf[i + 3] {
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = a;
        }
    });
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
    // No connection: the whole glyph is greyed out, with a dash instead of a level.
    let line = if l.tone == Tone::Dim { 0.55 } else { 1.0 };
    let dash = if l.fill.is_none() {
        format!(r#"<path d="M5.5 8h3" stroke="{outline}" stroke-opacity="{line}" stroke-width="1.5" stroke-linecap="round"/>"#)
    } else {
        String::new()
    };
    let level = l.fill.map(|v| v.min(100) as f32);
    let w = level.map(|v| if v > 0.0 { (10.0 * v / 100.0).max(1.0) } else { 0.0 }).unwrap_or(0.0);
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">
<rect x="0.5" y="4.5" width="13" height="7" rx="2" fill="none" stroke="{outline}" stroke-opacity="{line}" stroke-width="1"/>
<path d="M14.5 6.5v3" stroke="{outline}" stroke-opacity="{line}" stroke-width="1" stroke-linecap="round" fill="none"/>
<rect x="2" y="6" width="{w}" height="4" rx="0.8" fill="{fill}" fill-opacity="{opacity}"/>{dash}"#
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
pub fn render_battery_template(text: Option<&str>, fill: Option<u8>, charging: bool, size: &str) -> Image<'static> {
    const H: u32 = 54;
    let font = FontRef::try_from_slice(FONT).expect("font");
    // Digits 21 px tall at 3x (7 pt) for "m", like the system battery's percentage.
    let px = px_for_digit_height(&font, (21.0 * size_factor(size)).round());
    let text = text.filter(|_| fill.is_some());
    let text_w = text.map(|t| text_width(&font, t, px, 0.0)).unwrap_or(0.0);
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
<path d="M{dx0} 27h{dw}" stroke="black" stroke-opacity="0.6" stroke-width="3.5" stroke-linecap="round"/>
</svg>"#,
            dx0 = bx + body_w / 2.0 - 7.0,
            dw = 14.0,
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
    let baseline = centred_baseline(&font, px, by, bh);
    let x = draw_digits(&font, text, px, 0.0, bx + (body_w - content) / 2.0, baseline, (w, H), |x, y, cov| {
        let i = (y * w + x) as usize;
        punch[i] = punch[i].max(cov);
    });
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

/// macOS, voltage mode: a template image with a small bolt, the grid voltage and the
/// unit ("V", or "B" which doubles as the Cyrillic "В"), in the same sans-serif digits.
pub fn render_voltage_template(text: &str, unit: &str, size: &str) -> Image<'static> {
    const H: u32 = 54;
    let font = FontRef::try_from_slice(FONT).expect("font");
    // Digits 27 px tall at 3x (9 pt, like menu bar text) for "m", the unit a bit smaller.
    let f = size_factor(size);
    let px = px_for_digit_height(&font, (27.0 * f).round().min(39.0));
    let unit_px = px_for_digit_height(&font, (21.0 * f).round().min(30.0));
    let (bolt_w, gap, unit_gap) = (13.0f32, 5.0f32, 4.0f32);
    let text_w = text_width(&font, text, px, 0.0);
    let unit_w = text_width(&font, unit, unit_px, 0.0);
    let w = (3.0 + bolt_w + gap + text_w + unit_gap + unit_w + 3.0).ceil() as u32;
    let baseline = centred_baseline(&font, px, 0.0, H as f32);

    // Bolt first (BOLT spans x 5.2..10.3, y 3.2..12.8; 2.6x makes it 25 px tall).
    let (tx, ty) = (3.0 - 5.2 * 2.6, (H as f32 - 25.0) / 2.0 - 3.2 * 2.6);
    let bolt = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {H}" width="{w}" height="{H}"><path d="{BOLT}" fill="black" transform="translate({tx} {ty}) scale(2.6)"/></svg>"#
    );
    let mut buf = rasterize(&bolt, w, H).rgba().to_vec();
    let mut put = |x: u32, y: u32, cov: f32| {
        let i = ((y * w + x) * 4 + 3) as usize;
        buf[i] = buf[i].max((cov * 255.0) as u8);
    };
    let end = draw_digits(&font, text, px, 0.0, 3.0 + bolt_w + gap, baseline, (w, H), &mut put);
    draw_digits(&font, unit, unit_px, 0.0, end + unit_gap, baseline, (w, H), &mut put);
    Image::new_owned(buf, w, H)
}

/// The same image at reduced opacity (macOS "no connection" with the last level kept:
/// a template image has no colour, so it is greyed out by its alpha).
pub fn faded(img: Image<'static>, opacity: f32) -> Image<'static> {
    let (w, h) = (img.width(), img.height());
    let mut buf = img.rgba().to_vec();
    for a in buf.iter_mut().skip(3).step_by(4) {
        *a = (*a as f32 * opacity).round() as u8;
    }
    Image::new_owned(buf, w, h)
}

pub fn tooltip(s: &Snapshot, lang: &str) -> String {
    let t = i18n::get(lang);
    let name = if s.model.is_empty() { "EcoFlow".to_string() } else { s.model.clone() };
    match s.soc {
        None => format!("{name} – {}", t.no_data),
        Some(soc) if s.stale => format!("{name} – {}\n{soc}% ({})", t.lost, t.last_known),
        Some(soc) => {
            let mut out = format!("{name} – {soc}%");
            if s.charging == Some(true) {
                out.push_str(&format!(", {}", t.charging));
            }
            if let (Some(i), Some(o)) = (s.watts_in, s.watts_out) {
                out.push_str(&format!("\n↓ {i} {w}  ↑ {o} {w}", w = t.watts));
            }
            if let Some(v) = s.volts_in {
                out.push_str(&format!("\n{} {v} {}", t.grid, t.volts));
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
            let l = TrayLook { kind: Kind::Percent, text: "78".into(), tone: Tone::Normal, fill: Some(78), charging: false, light_bar: false, size: "m", px };
            for img in [render_number(&l), render_battery_color(&l)] {
                assert_eq!((img.width(), img.height()), (px, px));
                assert!(img.rgba().chunks(4).filter(|p| p[3] > 128).count() > (px * px / 12) as usize);
            }
        }
        // macOS: 18 pt tall at 3x; never narrower with more content; digits are holes.
        let t = render_battery_template(Some("50"), Some(50), false, "m");
        assert_eq!(t.height(), 54);
        let wide = render_battery_template(Some("100"), Some(100), true, "m");
        assert!(wide.width() >= t.width());
        let solid = |img: &Image| img.rgba().chunks(4).filter(|p| p[3] > 200).count();
        assert!(solid(&render_battery_template(None, Some(100), false, "m")) > solid(&render_battery_template(Some("100"), Some(100), false, "m")));
        // No data: an empty outline, no digits.
        let empty = render_battery_template(Some("–"), None, false, "m");
        assert_eq!(empty.width(), t.width());
        assert_eq!(solid(&empty), 0);
        assert_eq!(tray_px(1.0), if cfg!(windows) { 16 } else { 24 });
        let v = render_voltage_template("228", "V", "m");
        assert_eq!(v.height(), 54);
        assert!(v.width() > 60 && v.rgba().chunks(4).any(|p| p[3] > 200));
    }

    #[test]
    fn font_subset_has_every_glyph_we_draw() {
        for data in [FONT, FONT_REGULAR] {
            let font = FontRef::try_from_slice(data).unwrap();
            for ch in "0123456789–VB".chars() {
                assert_ne!(font.glyph_id(ch).0, 0, "missing glyph for {ch:?}");
                assert!(font.as_scaled(PxScale::from(20.0)).h_advance(font.glyph_id(ch)) > 1.0);
            }
        }
    }

    #[test]
    fn digit_size_setting_scales_every_icon() {
        // Windows: one pixel per step at 100%, and still fits at the largest size.
        let heights: Vec<f32> = ["s", "m", "l", "xl"].iter().map(|z| tray_digit_height(16, z)).collect();
        assert_eq!(heights, vec![8.0, 9.0, 10.0, 11.0]);
        assert_eq!(tray_digit_height(16, "nonsense"), 9.0);
        for z in ["s", "m", "l", "xl"] {
            for text in ["78", "100", "228"] {
                let l = TrayLook { kind: Kind::Percent, text: text.into(), tone: Tone::Normal, fill: None, charging: false, light_bar: false, size: digit_size(z), px: 16 };
                let (rows, cols) = ink(&render_number(&l));
                assert!(!rows.is_empty() && !cols.is_empty(), "{text}/{z} drew nothing");
            }
        }
        let tall = |img: &Image| ink(img).0.len();
        let two = |z: &str| TrayLook { kind: Kind::Percent, text: "78".into(), tone: Tone::Normal, fill: None, charging: false, light_bar: false, size: digit_size(z), px: 16 };
        assert!(tall(&render_number(&two("s"))) < tall(&render_number(&two("xl"))));
        // macOS: bigger digits, wider (or equal) items; always 18 pt tall.
        let small = render_battery_template(Some("78"), Some(78), false, "s");
        let big = render_battery_template(Some("100"), Some(100), true, "xl");
        assert_eq!((small.height(), big.height()), (54, 54));
        assert!(big.width() >= small.width());
        assert!(render_voltage_template("228", "B", "xl").width() > render_voltage_template("228", "B", "s").width());
    }

    fn snap(soc: Option<u8>, volts: Option<i64>) -> Snapshot {
        Snapshot { soc, volts_in: volts, ..Default::default() }
    }

    #[test]
    fn lost_connection_shows_a_dash_or_the_dimmed_last_level() {
        let lost = Snapshot { soc: Some(99), stale: true, ..Default::default() };
        let l = look(&lost, 20, 16, "number", "m", "dash");
        assert_eq!((l.text.as_str(), l.tone, l.fill), ("–", Tone::Dim, None));
        let l = look(&lost, 20, 16, "number", "m", "dim");
        assert_eq!((l.text.as_str(), l.tone, l.fill), ("99", Tone::Dim, Some(99)));
        // Unknown setting values behave like the default.
        assert_eq!(look(&lost, 20, 16, "number", "m", "?").text, "–");
        // Voltage mode: no live voltage when lost, so the same rule applies.
        let l = look(&Snapshot { soc: Some(99), stale: true, volts_in: None, ..Default::default() }, 20, 16, "voltage", "m", "dash");
        assert_eq!((l.kind, l.text.as_str()), (Kind::Percent, "–"));
        // Connected: the level as usual.
        assert_eq!(look(&Snapshot { soc: Some(99), ..Default::default() }, 20, 16, "number", "m", "dash").text, "99");
        // macOS dimming keeps the shape and halves the alpha.
        let t = render_battery_template(Some("99"), Some(99), false, "m");
        let f = faded(render_battery_template(Some("99"), Some(99), false, "m"), 0.5);
        let max = |img: &Image| img.rgba().chunks(4).map(|p| p[3]).max().unwrap();
        assert_eq!((f.width(), f.height()), (t.width(), t.height()));
        assert!(max(&f) <= max(&t) / 2 + 1);
    }

    #[test]
    fn voltage_style_shows_volts_and_falls_back_to_percent() {
        let l = look(&snap(Some(78), Some(228)), 20, 16, "voltage", "m", "dash");
        assert_eq!((l.kind, l.text.as_str(), l.tone), (Kind::Voltage, "228", Tone::Normal));
        // No grid, or far out of range: flagged.
        assert_eq!(look(&snap(Some(78), Some(0)), 20, 16, "voltage", "m", "dash").tone, Tone::Low);
        assert_eq!(look(&snap(Some(78), Some(262)), 20, 16, "voltage", "m", "dash").tone, Tone::Low);
        assert_eq!(look(&snap(Some(78), Some(205)), 20, 16, "voltage", "m", "dash").tone, Tone::Normal);
        // No reading (cloud off): the percentage instead of a blank item.
        let l = look(&snap(Some(78), None), 20, 16, "voltage", "m", "dash");
        assert_eq!((l.kind, l.text.as_str()), (Kind::Percent, "78"));
        // Other styles never show volts.
        assert_eq!(look(&snap(Some(78), Some(228)), 20, 16, "number", "m", "dash").kind, Kind::Percent);
        assert_eq!(look(&snap(None, None), 20, 16, "voltage", "m", "dash").text, "–");
    }

    fn ink(img: &Image) -> (Vec<u32>, Vec<u32>) {
        let (w, h) = (img.width(), img.height());
        let rgba = img.rgba();
        let rows = (0..h).filter(|y| (0..w).any(|x| rgba[((y * w + x) * 4 + 3) as usize] > 90)).collect();
        let cols = (0..w).filter(|x| (0..h).any(|y| rgba[((y * w + x) * 4 + 3) as usize] > 90)).collect();
        (rows, cols)
    }

    #[test]
    fn digits_match_the_taskbar_text_and_fit() {
        for px in [16u32, 20, 24, 32] {
            let target = tray_digit_height(px, "m") as u32;
            let mk = |text: &str| TrayLook { kind: Kind::Percent, text: text.into(), tone: Tone::Normal, fill: None, charging: false, light_bar: false, size: "m", px };
            // One and two digits: the taskbar text height (±1 px of anti-aliasing), centred.
            for text in ["5", "14", "78", "88"] {
                let (rows, cols) = ink(&render_number(&mk(text)));
                let height = rows.last().unwrap() - rows.first().unwrap() + 1;
                assert!(height.abs_diff(target) <= 1, "{text}@{px}: {height} vs {target}");
                let (top, bottom) = (*rows.first().unwrap(), px - 1 - rows.last().unwrap());
                assert!(top.abs_diff(bottom) <= 1, "{text}@{px}: top {top} bottom {bottom}");
                assert!(*cols.first().unwrap() > 0 && *cols.last().unwrap() < px - 1, "{text}@{px} touches the edge");
            }
            // Three digits: never clipped, never bigger than two, and still readable.
            for text in ["100", "228", "253"] {
                let (rows, cols) = ink(&render_number(&mk(text)));
                let height = rows.last().unwrap() - rows.first().unwrap() + 1;
                assert!(height <= target + 1 && height * 100 >= target * 68, "{text}@{px}: {height} vs {target}");
                assert!(cols.len() as u32 >= px * 3 / 4, "{text}@{px}: only {} columns used", cols.len());
            }
        }
    }

    #[test]
    fn fit_digits_tightens_before_shrinking() {
        let font = FontRef::try_from_slice(FONT_REGULAR).unwrap();
        let full = px_for_digit_height(&font, 9.0);
        // Two digits fit with the wanted spacing; one digit has no gaps to space.
        assert_eq!(fit_digits(&font, "78", 9.0, 14.0, 1.0), (full, 1.0));
        assert_eq!(fit_digits(&font, "5", 9.0, 14.0, 1.0), (full, 0.0));
        // Wider spacing than fits is reduced, the size stays.
        let (px, tr) = fit_digits(&font, "88", 9.0, 14.0, 4.0);
        assert_eq!(px, full);
        assert!(tr < 4.0 && ink_span(&font, "88", px, tr).1 <= 14.0 + 1e-3);
        // Three digits: spacing first (never more than 9% per gap below zero), then size.
        let (px, tr) = fit_digits(&font, "100", 9.0, 16.0, 1.0);
        assert!(tr <= 0.0 && tr >= -0.09 * px - 1e-4);
        assert!(px <= full);
        assert!(ink_span(&font, "100", px, tr).1 <= 16.0 + 1e-3);
        assert_eq!((tray_spacing(16), tray_spacing(24), tray_spacing(32)), (0.5, 1.0, 1.0));
    }

    #[test]
    fn two_digits_are_spaced_out() {
        // The gap between the two digits of "99" is wider than with the font's default.
        let mk = |px| TrayLook { kind: Kind::Percent, text: "99".into(), tone: Tone::Normal, fill: None, charging: false, light_bar: false, size: "m", px };
        let img = render_number(&mk(16));
        let (_, cols) = ink(&img);
        let longest_gap = cols.windows(2).map(|w| w[1] - w[0] - 1).max().unwrap_or(0);
        assert!(longest_gap >= 1, "no visible gap between the digits: {cols:?}");
        // ...and the pair is still inside the 1 px margin.
        assert!(cols[0] >= 1 && *cols.last().unwrap() <= 14, "{cols:?}");
    }

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
                    ("99", Tone::Dim, Some(99), false),
                ] {
                    let l = TrayLook { kind: Kind::Percent, text: text.into(), tone, fill, charging, light_bar, size: "m", px };
                    for (kind, img) in [("num", render_number(&l)), ("bat", render_battery_color(&l))] {
                        let theme = if light_bar { "light" } else { "dark" };
                        let path = format!("{dir}/{px:02}-{theme}-{n:03}-{kind}-{}x{}.rgba", img.width(), img.height());
                        std::fs::write(path, img.rgba()).unwrap();
                    }
                    n += 1;
                }
            }
        }
        for (j, img) in [
            render_battery_template(Some("99"), Some(99), false, "m"),
            faded(render_battery_template(Some("99"), Some(99), false, "m"), 0.45),
            render_battery_template(Some("–"), None, false, "m"),
        ].into_iter().enumerate() {
            std::fs::write(format!("{dir}/mac-lost-{j}-{}x{}.rgba", img.width(), img.height()), img.rgba()).unwrap();
        }
        for px in [16u32, 24] {
            for (j, (v, tone)) in [("228", Tone::Normal), ("0", Tone::Low), ("253", Tone::Normal)].iter().enumerate() {
                let l = TrayLook { kind: Kind::Voltage, text: (*v).into(), tone: *tone, fill: None, charging: false, light_bar: false, size: "m", px };
                let img = render_number(&l);
                std::fs::write(format!("{dir}/volt-{px:02}-{j}-{}x{}.rgba", img.width(), img.height()), img.rgba()).unwrap();
            }
        }
        for px in [16u32, 24] {
            for z in ["s", "m", "l", "xl"] {
                for (k, text) in ["78", "100", "14", "228"].iter().enumerate() {
                    for light_bar in [false, true] {
                        let l = TrayLook { kind: Kind::Percent, text: (*text).into(), tone: Tone::Normal, fill: None, charging: false, light_bar, size: z, px };
                        let img = render_number(&l);
                        let theme = if light_bar { "light" } else { "dark" };
                        std::fs::write(format!("{dir}/size-{px:02}-{z}-{theme}-{k}-{}x{}.rgba", img.width(), img.height()), img.rgba()).unwrap();
                    }
                }
            }
        }
        for (j, (v, u)) in [("228", "V"), ("228", "B"), ("0", "B")].iter().enumerate() {
            let t = render_voltage_template(v, u, "m");
            std::fs::write(format!("{dir}/macv-{j}-{}x{}.rgba", t.width(), t.height()), t.rgba()).unwrap();
        }
        for (i, (fill, ch)) in [(Some(78u8), false), (Some(78), true), (Some(14), false), (Some(100), false), (Some(100), true), (Some(5), true), (None, false)].iter().enumerate() {
            let text = fill.map(|v| v.to_string());
            let t = render_battery_template(text.as_deref(), *fill, *ch, "m");
            std::fs::write(format!("{dir}/mac-{i}-{}x{}.rgba", t.width(), t.height()), t.rgba()).unwrap();
        }
    }
}
