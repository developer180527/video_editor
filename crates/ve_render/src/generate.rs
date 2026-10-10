//! The built-in generators — colour matte, bars, titles — drawn on the CPU
//! as display-referred RGBA (Rec.709 primaries, BT.1886 encoding, straight
//! alpha), the way a colour picker or a graphics package means its colours.
//! The compositor's RGBA path brings them into the working space like any
//! footage, so 75% bars measure 75% whatever the working space.
//!
//! Pictures are cached by their parameters: an unchanging title is drawn
//! once and the compositor sees the same frame (and skips the upload).

use std::sync::{Arc, Mutex, OnceLock};

use ve_model::Value;
use ve_ports::{ColorTags, FrameData, PixelFormat, VideoFrame};
use ve_time::Time;

/// Inter (SIL Open Font License 1.1, from libgui). An app that ships this
/// must ship `third_party/libgui/assets/Inter-OFL.txt` with it.
static FONT_DATA: &[u8] = include_bytes!("../../../third_party/libgui/assets/Inter.ttf");

fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    FONT.get_or_init(|| fontdue::Font::from_bytes(FONT_DATA, fontdue::FontSettings::default()).expect("Inter.ttf"))
}

/// The generator `id`'s picture at `w`×`h` for these parameter values, or
/// `None` if `id` is not a built-in generator. `scale` is picture pixels
/// per sequence pixel (below 1 at preview quality): sizes and positions in
/// the parameters are in sequence pixels.
pub fn picture(id: &str, params: &[(String, Value)], w: u32, h: u32, scale: f32) -> Option<Arc<VideoFrame>> {
    let key = format!("{id} {w}x{h} {scale} {params:?}");
    /// The most recent pictures, newest first, by parameters.
    type Cache = Mutex<Vec<(String, Arc<VideoFrame>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, f)) = cache.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return Some(f.clone());
    }
    let get = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let color = |name: &str, d: [f32; 4]| match get(name) {
        Some(Value::Color(c)) => c,
        _ => d,
    };
    let rgba = match id {
        "ve.color" => solid(w, h, color("color", [0.5, 0.5, 0.5, 1.0])),
        "ve.bars" => bars(w, h),
        "ve.title" => title(w, h, &TitleStyle::from_params(params), scale),
        _ => return None,
    };
    let frame = Arc::new(VideoFrame {
        pts: Time::ZERO,
        duration: Time::MAX,
        width: w,
        height: h,
        format: PixelFormat::Rgba8,
        color: ColorTags { primaries: "bt709".into(), transfer: "bt709".into(), matrix: String::new(), full_range: true },
        data: FrameData::Cpu { planes: vec![rgba], strides: vec![w as usize * 4] },
    });
    let mut c = cache.lock().unwrap();
    c.insert(0, (key, frame.clone()));
    c.truncate(16);
    Some(frame)
}

/// [`picture`]'s pixels as straight RGBA8 rows (for a UI thumbnail).
pub fn picture_rgba(id: &str, params: &[(String, Value)], w: u32, h: u32, scale: f32) -> Option<Vec<u8>> {
    match &picture(id, params, w, h, scale)?.data {
        FrameData::Cpu { planes, .. } => Some(planes[0].clone()),
        _ => None,
    }
}

/// Linear-light 0..1 → a display-encoded byte (BT.1886, 2.4).
fn encode(v: f32) -> u8 {
    (v.clamp(0.0, 1.0).powf(1.0 / 2.4) * 255.0).round() as u8
}

fn solid(w: u32, h: u32, c: [f32; 4]) -> Vec<u8> {
    let px = [encode(c[0]), encode(c[1]), encode(c[2]), (c[3].clamp(0.0, 1.0) * 255.0).round() as u8];
    px.repeat((w * h) as usize)
}

/// SMPTE-style 75% colour bars: white, yellow, cyan, green, magenta, red,
/// blue across the top two thirds; then the reverse-blue row; then black
/// with a -I/white/+Q-less simplified PLUGE (black, 100% white, black).
fn bars(w: u32, h: u32) -> Vec<u8> {
    const L: u8 = 191; // 75% (display-encoded, as bars are specified)
    let top = [[L, L, L], [L, L, 0], [0, L, L], [0, L, 0], [L, 0, L], [L, 0, 0], [0, 0, L]];
    let mid = [[0, 0, L], [0, 0, 0], [L, 0, L], [0, 0, 0], [0, L, L], [0, 0, 0], [L, L, L]];
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let bar = (x * 7 / w.max(1)) as usize;
            let c = if y < h * 2 / 3 {
                top[bar]
            } else if y < h * 3 / 4 {
                mid[bar]
            } else if (w / 4..w / 2).contains(&x) {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            };
            out.extend([c[0], c[1], c[2], 255]);
        }
    }
    out
}

/// How a title looks, from its parameters. Sizes and positions are in
/// sequence pixels; colours display-referred (what a picker shows).
#[derive(Clone, Debug, PartialEq)]
pub struct TitleStyle {
    pub text: String,
    pub size: f32,
    pub color: [f32; 4],
    /// The anchor: the block's left edge, centre or right edge (by
    /// `align`), at its vertical centre. `None`: the picture's centre.
    pub position: Option<[f32; 2]>,
    /// 0 left, 1 centre, 2 right.
    pub align: u32,
    /// Extra space between letters, thousandths of the size.
    pub tracking: f32,
    /// Line spacing, percent of the font's.
    pub leading: f32,
    /// Heavier strokes (Inter ships one weight; this thickens it).
    pub bold: bool,
    /// Outline: colour and width (px).
    pub stroke: Option<([f32; 4], f32)>,
    /// A box behind the text: colour and padding (px).
    pub boxed: Option<([f32; 4], f32)>,
    /// Drop shadow: colour, distance and softness (px), down and right.
    pub shadow: Option<([f32; 4], f32, f32)>,
}

impl TitleStyle {
    pub fn from_params(params: &[(String, Value)]) -> TitleStyle {
        let get = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let f = |name: &str, d: f32| match get(name) {
            Some(Value::Float(v)) => v as f32,
            Some(Value::Int(v)) => v as f32,
            _ => d,
        };
        let on = |name: &str| matches!(get(name), Some(Value::Bool(true)));
        let c = |name: &str, d: [f32; 4]| match get(name) {
            Some(Value::Color(c)) => c,
            _ => d,
        };
        TitleStyle {
            text: match get("text") {
                Some(Value::Text(t)) => t,
                _ => "Title".into(),
            },
            size: f("size", 96.0),
            color: c("color", [1.0; 4]),
            position: match get("position") {
                Some(Value::Vec2([x, y])) => Some([x as f32, y as f32]),
                _ => None,
            },
            align: match get("align") {
                Some(Value::Choice(a)) => a.min(2),
                _ => 1,
            },
            tracking: f("tracking", 0.0),
            leading: f("leading", 100.0),
            bold: on("bold"),
            stroke: on("stroke").then(|| (c("stroke_color", [0.0, 0.0, 0.0, 1.0]), f("stroke_width", 4.0))),
            boxed: on("box").then(|| (c("box_color", [0.0, 0.0, 0.0, 0.6]), f("box_padding", 24.0))),
            shadow: on("shadow").then(|| (c("shadow_color", [0.0, 0.0, 0.0, 0.75]), f("shadow_distance", 6.0), f("shadow_softness", 4.0))),
        }
    }
}

/// A title laid out at `scale` (picture px per sequence px): each line's
/// glyphs and pen positions, and the text block's rectangle.
struct Layout {
    /// (character, x of its pen, baseline y).
    glyphs: Vec<(char, f32, f32)>,
    size: f32,
    /// x, y, w, h of the text block.
    block: [f32; 4],
}

fn layout(s: &TitleStyle, scale: f32, picture: (f32, f32)) -> Layout {
    let f = font();
    let size = (s.size * scale).max(1.0);
    let metrics = f.horizontal_line_metrics(size);
    let line_h = metrics.map_or(size * 1.2, |m| m.new_line_size) * (s.leading / 100.0).max(0.1);
    let ascent = metrics.map_or(size * 0.8, |m| m.ascent);
    let descent = metrics.map_or(-size * 0.2, |m| m.descent);
    let track = s.tracking / 1000.0 * size;
    let lines: Vec<&str> = s.text.split('\n').collect();
    let width = |line: &str| {
        let n = line.chars().count();
        line.chars().map(|ch| f.metrics(ch, size).advance_width).sum::<f32>() + track * n.saturating_sub(1) as f32
    };
    let widths: Vec<f32> = lines.iter().map(|l| width(l)).collect();
    let block_w = widths.iter().cloned().fold(0.0, f32::max);
    // The block: its lines' spacing, with the last line's ascent to descent.
    let block_h = line_h * (lines.len() as f32 - 1.0) + (ascent - descent);
    let (px, py) = s.position.map_or((picture.0 / 2.0, picture.1 / 2.0), |p| (p[0] * scale, p[1] * scale));
    let left = match s.align {
        0 => px,
        2 => px - block_w,
        _ => px - block_w / 2.0,
    };
    let top = py - block_h / 2.0;
    let mut glyphs = Vec::new();
    for (i, (line, lw)) in lines.iter().zip(&widths).enumerate() {
        let mut pen = match s.align {
            0 => left,
            2 => left + block_w - lw,
            _ => left + (block_w - lw) / 2.0,
        };
        let baseline = top + i as f32 * line_h + ascent;
        for ch in line.chars() {
            glyphs.push((ch, pen, baseline));
            pen += f.metrics(ch, size).advance_width + track;
        }
    }
    Layout { glyphs, size, block: [left, top, block_w, block_h] }
}

/// The title's text block in sequence pixels (x, y, w, h), padding and
/// effects excluded, in a sequence `seq` big: what the monitor outlines and
/// drags.
pub fn title_bounds(s: &TitleStyle, seq: (u32, u32)) -> [f32; 4] {
    layout(s, 1.0, (seq.0 as f32, seq.1 as f32)).block
}

/// A rectangle of the picture, as pixel bounds clamped to it.
#[derive(Clone, Copy)]
struct Region {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Region {
    fn around(r: [f32; 4], margin: f32, w: u32, h: u32) -> Region {
        let c = |v: f32, max: u32| (v.floor().max(0.0) as usize).min(max as usize);
        Region { x0: c(r[0] - margin, w), y0: c(r[1] - margin, h), x1: c(r[0] + r[2] + margin + 1.0, w), y1: c(r[1] + r[3] + margin + 1.0, h) }
    }
}

/// Grow `mask` by `r` px within `reg` (a square max filter, in two passes).
fn dilate(mask: &[f32], w: usize, reg: Region, r: usize) -> Vec<f32> {
    if r == 0 {
        return mask.to_vec();
    }
    let mut tmp = mask.to_vec();
    for y in reg.y0..reg.y1 {
        for x in reg.x0..reg.x1 {
            let (a, b) = (x.saturating_sub(r).max(reg.x0), (x + r + 1).min(reg.x1));
            tmp[y * w + x] = mask[y * w + a..y * w + b].iter().cloned().fold(0.0, f32::max);
        }
    }
    let mut out = tmp.clone();
    for y in reg.y0..reg.y1 {
        for x in reg.x0..reg.x1 {
            let (a, b) = (y.saturating_sub(r).max(reg.y0), (y + r + 1).min(reg.y1));
            out[y * w + x] = (a..b).map(|yy| tmp[yy * w + x]).fold(0.0, f32::max);
        }
    }
    out
}

/// Grow `mask` by a fractional `r` px: between the whole-pixel growths
/// either side, so a thin outline or a faux bold scales smoothly with the
/// picture (a half-size preview looks like the full-size picture).
fn dilate_by(mask: &[f32], w: usize, reg: Region, r: f32) -> Vec<f32> {
    let r = r.max(0.0);
    let (whole, frac) = (r.floor() as usize, r.fract());
    let lo = dilate(mask, w, reg, whole);
    if frac < 0.01 {
        return lo;
    }
    let hi = dilate(mask, w, reg, whole + 1);
    lo.iter().zip(&hi).map(|(a, b)| a + (b - a) * frac).collect()
}

/// Soften `mask` within `reg`: two box blurs of radius `r`, close to a
/// Gaussian.
fn blur(mask: &[f32], w: usize, reg: Region, r: usize) -> Vec<f32> {
    let mut m = mask.to_vec();
    if r == 0 {
        return m;
    }
    for _ in 0..2 {
        let mut tmp = m.clone();
        for y in reg.y0..reg.y1 {
            for x in reg.x0..reg.x1 {
                let (a, b) = (x.saturating_sub(r).max(reg.x0), (x + r + 1).min(reg.x1));
                tmp[y * w + x] = m[y * w + a..y * w + b].iter().sum::<f32>() / (2 * r + 1) as f32;
            }
        }
        for y in reg.y0..reg.y1 {
            for x in reg.x0..reg.x1 {
                let (a, b) = (y.saturating_sub(r).max(reg.y0), (y + r + 1).min(reg.y1));
                m[y * w + x] = (a..b).map(|yy| tmp[yy * w + x]).sum::<f32>() / (2 * r + 1) as f32;
            }
        }
    }
    m
}

/// The title over transparency: box, shadow, outline, then the text.
fn title(w: u32, h: u32, s: &TitleStyle, scale: f32) -> Vec<u8> {
    let f = font();
    let lay = layout(s, scale, (w as f32, h as f32));
    let (wu, hu) = (w as usize, h as usize);
    let px = |v: f32| (v * scale).round().max(0.0) as usize;
    let bold = if s.bold { lay.size * 0.03 } else { 0.0 };
    let stroke = s.stroke.map_or(0.0, |(_, sw)| sw * scale);
    let (sh_dist, sh_soft) = s.shadow.map_or((0, 0), |(_, d, soft)| (px(d), px(soft)));
    let pad = s.boxed.map_or(0.0, |(_, p)| p * scale);
    // Everything happens within the block, grown by what reaches past it.
    let margin = bold + stroke + (sh_dist + 2 * sh_soft) as f32 + pad + lay.size * 0.3 + 2.0;
    let reg = Region::around(lay.block, margin, w, h);

    // The text's coverage.
    let mut mask = vec![0f32; wu * hu];
    for &(ch, pen, baseline) in &lay.glyphs {
        let (m, cov) = f.rasterize(ch, lay.size);
        let x0 = (pen + m.xmin as f32).round() as i64;
        let y0 = (baseline - m.height as f32 - m.ymin as f32).round() as i64;
        for gy in 0..m.height {
            for gx in 0..m.width {
                let (x, y) = (x0 + gx as i64, y0 + gy as i64);
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let i = y as usize * wu + x as usize;
                mask[i] = mask[i].max(cov[gy * m.width + gx] as f32 / 255.0);
            }
        }
    }
    let fill = dilate_by(&mask, wu, reg, bold);
    let outline = s.stroke.map(|_| dilate_by(&fill, wu, reg, stroke));

    // Composite, premultiplied, in display-encoded values.
    let mut out = vec![[0f32; 4]; wu * hu];
    let over = |dst: &mut [f32; 4], c: [f32; 4], a: f32| {
        let a = (a * c[3]).clamp(0.0, 1.0);
        let rgb = [encode(c[0]) as f32 / 255.0, encode(c[1]) as f32 / 255.0, encode(c[2]) as f32 / 255.0];
        for k in 0..3 {
            dst[k] = rgb[k] * a + dst[k] * (1.0 - a);
        }
        dst[3] = a + dst[3] * (1.0 - a);
    };
    if let Some((c, _)) = s.boxed {
        let b = [lay.block[0] - pad, lay.block[1] - pad, lay.block[2] + 2.0 * pad, lay.block[3] + 2.0 * pad];
        let r = Region::around(b, 0.0, w, h);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                // Soft edges: partial coverage at the rectangle's sides.
                let cx = ((x as f32 + 1.0).min(b[0] + b[2]) - (x as f32).max(b[0])).clamp(0.0, 1.0);
                let cy = ((y as f32 + 1.0).min(b[1] + b[3]) - (y as f32).max(b[1])).clamp(0.0, 1.0);
                over(&mut out[y * wu + x], c, cx * cy);
            }
        }
    }
    if let Some((c, _, _)) = s.shadow {
        let src = outline.as_ref().unwrap_or(&fill);
        let soft = blur(src, wu, reg, sh_soft);
        // Down and to the right, at 45°.
        let d = (sh_dist as f32 / std::f32::consts::SQRT_2).round() as usize;
        for y in reg.y0..reg.y1 {
            for x in reg.x0..reg.x1 {
                if x >= reg.x0 + d && y >= reg.y0 + d {
                    let a = soft[(y - d) * wu + (x - d)];
                    if a > 0.0 {
                        over(&mut out[y * wu + x], c, a);
                    }
                }
            }
        }
    }
    if let (Some((c, _)), Some(o)) = (s.stroke, &outline) {
        for y in reg.y0..reg.y1 {
            for x in reg.x0..reg.x1 {
                let a = o[y * wu + x];
                if a > 0.0 {
                    over(&mut out[y * wu + x], c, a);
                }
            }
        }
    }
    for y in reg.y0..reg.y1 {
        for x in reg.x0..reg.x1 {
            let a = fill[y * wu + x];
            if a > 0.0 {
                over(&mut out[y * wu + x], s.color, a);
            }
        }
    }
    // Straight alpha, as the picture is uploaded.
    let mut bytes = vec![0u8; wu * hu * 4];
    for (i, p) in out.iter().enumerate() {
        if p[3] > 0.0 {
            for k in 0..3 {
                bytes[i * 4 + k] = (p[k] / p[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            bytes[i * 4 + 3] = (p[3] * 255.0).round() as u8;
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(f: &VideoFrame, x: u32, y: u32) -> [u8; 4] {
        let FrameData::Cpu { planes, .. } = &f.data else { panic!() };
        let i = ((y * f.width + x) * 4) as usize;
        [planes[0][i], planes[0][i + 1], planes[0][i + 2], planes[0][i + 3]]
    }

    #[test]
    fn bars_matte_and_title_draw() {
        let b = picture("ve.bars", &[], 700, 300, 1.0).unwrap();
        assert_eq!(px(&b, 50, 10), [191, 191, 191, 255], "75% white");
        assert_eq!(px(&b, 150, 10), [191, 191, 0, 255], "yellow");
        let m = picture("ve.color", &[("color".into(), Value::Color([1.0, 0.0, 0.0, 1.0]))], 8, 8, 1.0).unwrap();
        assert_eq!(px(&m, 3, 3), [255, 0, 0, 255]);
        let t = picture("ve.title", &[("text".into(), Value::Text("IIII".into())), ("size".into(), Value::Float(64.0))], 400, 200, 1.0).unwrap();
        let FrameData::Cpu { planes, .. } = &t.data else { panic!() };
        let inked = planes[0].chunks(4).filter(|p| p[3] > 128).count();
        assert!(inked > 200, "the text is drawn ({inked} px)");
        assert_eq!(px(&t, 5, 5)[3], 0, "transparent around it");
        // Cached: the same parameters give the same frame.
        assert!(Arc::ptr_eq(&b, &picture("ve.bars", &[], 700, 300, 1.0).unwrap()));
        assert!(picture("com.example.unknown", &[], 4, 4, 1.0).is_none());
        // At half resolution a title centred at sequence (200, 100) is
        // centred at (100, 50), half the size.
        let p = [("text".into(), Value::Text("IIII".into())), ("size".into(), Value::Float(64.0)), ("position".into(), Value::Vec2([200.0, 100.0]))];
        let half = picture("ve.title", &p, 200, 100, 0.5).unwrap();
        let FrameData::Cpu { planes, .. } = &half.data else { panic!() };
        let xs: Vec<u32> = planes[0].chunks(4).enumerate().filter(|(_, p)| p[3] > 128).map(|(i, _)| i as u32 % 200).collect();
        let mid = (xs.iter().min().unwrap() + xs.iter().max().unwrap()) / 2;
        assert!((95..=105).contains(&mid), "centred at {mid}");
        assert!(xs.len() * 4 < inked + 40, "drawn at half size");
    }

    fn style(extra: &[(&str, Value)]) -> Vec<(String, Value)> {
        let mut p = vec![("text".to_string(), Value::Text("HHHH".into())), ("size".to_string(), Value::Float(60.0)), ("position".to_string(), Value::Vec2([200.0, 100.0]))];
        // An override replaces the base value of the same name.
        p.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
        p.extend(extra.iter().map(|(k, v)| (k.to_string(), v.clone())));
        p
    }

    fn alpha(f: &VideoFrame, x: u32, y: u32) -> u8 {
        px(f, x, y)[3]
    }

    #[test]
    fn titles_align_on_their_anchor() {
        let left = TitleStyle::from_params(&style(&[("align", Value::Choice(0))]));
        let right = TitleStyle::from_params(&style(&[("align", Value::Choice(2))]));
        let centre = TitleStyle::from_params(&style(&[]));
        let (l, r, c) = (title_bounds(&left, (400, 200)), title_bounds(&right, (400, 200)), title_bounds(&centre, (400, 200)));
        assert!((l[0] - 200.0).abs() < 0.01, "left edge on the anchor");
        assert!((r[0] + r[2] - 200.0).abs() < 0.01, "right edge on the anchor");
        assert!((c[0] + c[2] / 2.0 - 200.0).abs() < 0.01, "centred on it");
        // Two lines are taller than one; tracking widens.
        let two = TitleStyle::from_params(&style(&[("text", Value::Text("HH\nHH".into()))]));
        assert!(title_bounds(&two, (400, 200))[3] > c[3] * 1.5);
        let wide = TitleStyle::from_params(&style(&[("tracking", Value::Float(200.0))]));
        assert!(title_bounds(&wide, (400, 200))[2] > c[2] + 20.0);
    }

    #[test]
    fn box_stroke_and_shadow_draw_where_they_should() {
        let plain = picture("ve.title", &style(&[]), 400, 200, 1.0).unwrap();
        let b = title_bounds(&TitleStyle::from_params(&style(&[])), (400, 200));
        // Above the text, inside the padding: only the box fills it.
        let above = (b[0] as u32 + 4, (b[1] - 10.0) as u32);
        assert_eq!(alpha(&plain, above.0, above.1), 0);
        let boxed = picture("ve.title", &style(&[("box", Value::Bool(true)), ("box_padding", Value::Float(20.0))]), 400, 200, 1.0).unwrap();
        assert!(alpha(&boxed, above.0, above.1) > 100, "the box is behind it");
        // An outline covers more than the text alone.
        let inked = |f: &VideoFrame| {
            let FrameData::Cpu { planes, .. } = &f.data else { panic!() };
            planes[0].chunks(4).filter(|p| p[3] > 128).count()
        };
        let stroked = picture("ve.title", &style(&[("stroke", Value::Bool(true)), ("stroke_width", Value::Float(4.0))]), 400, 200, 1.0).unwrap();
        assert!(inked(&stroked) > inked(&plain) * 3 / 2, "{} vs {}", inked(&stroked), inked(&plain));
        let bold = picture("ve.title", &style(&[("bold", Value::Bool(true))]), 400, 200, 1.0).unwrap();
        assert!(inked(&bold) > inked(&plain));
        // A hard shadow lands down and right of the text.
        let shadowed = picture("ve.title", &style(&[("shadow", Value::Bool(true)), ("shadow_distance", Value::Float(12.0)), ("shadow_softness", Value::Float(0.0))]), 400, 200, 1.0).unwrap();
        // The text's bottom-right inked pixel; the shadow lies 12/√2 ≈ 8 px
        // down and right of it, where the text alone leaves nothing.
        let (mut cx, mut cy) = (0, 0);
        for y in 0..200 {
            for x in 0..400 {
                if alpha(&plain, x, y) > 200 && x + y >= cx + cy {
                    (cx, cy) = (x, y);
                }
            }
        }
        assert_eq!(alpha(&plain, cx + 8, cy + 8), 0);
        assert!(alpha(&shadowed, cx + 8, cy + 8) > 60, "a shadow down-right");
    }
}