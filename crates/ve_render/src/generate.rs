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
/// `None` if `id` is not a built-in generator.
pub fn picture(id: &str, params: &[(String, Value)], w: u32, h: u32) -> Option<Arc<VideoFrame>> {
    let key = format!("{id} {w}x{h} {params:?}");
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
        "ve.title" => {
            let text = match get("text") {
                Some(Value::Text(t)) => t,
                _ => "Title".into(),
            };
            let size = match get("size") {
                Some(Value::Float(s)) => s as f32,
                _ => 96.0,
            };
            let at = match get("position") {
                Some(Value::Vec2([x, y])) => [x as f32, y as f32],
                _ => [w as f32 / 2.0, h as f32 / 2.0],
            };
            title(w, h, &text, size, color("color", [1.0; 4]), at)
        }
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

/// `text` centred on `at`, one line per `\n`, over transparency.
fn title(w: u32, h: u32, text: &str, size: f32, c: [f32; 4], at: [f32; 2]) -> Vec<u8> {
    let f = font();
    let mut out = vec![0u8; (w * h * 4) as usize];
    let lines: Vec<&str> = text.lines().collect();
    let metrics = f.horizontal_line_metrics(size);
    let line_h = metrics.map_or(size * 1.2, |m| m.new_line_size);
    let ascent = metrics.map_or(size * 0.8, |m| m.ascent);
    let top = at[1] - line_h * lines.len() as f32 / 2.0;
    let rgb = [encode(c[0]), encode(c[1]), encode(c[2])];
    for (li, line) in lines.iter().enumerate() {
        let width: f32 = line.chars().map(|ch| f.metrics(ch, size).advance_width).sum();
        let mut pen = at[0] - width / 2.0;
        let baseline = top + li as f32 * line_h + ascent;
        for ch in line.chars() {
            let (m, cov) = f.rasterize(ch, size);
            let x0 = (pen + m.xmin as f32).round() as i64;
            let y0 = (baseline - m.height as f32 - m.ymin as f32).round() as i64;
            for gy in 0..m.height {
                for gx in 0..m.width {
                    let (x, y) = (x0 + gx as i64, y0 + gy as i64);
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        continue;
                    }
                    let a = cov[gy * m.width + gx] as f32 / 255.0 * c[3];
                    let i = ((y as u32 * w + x as u32) * 4) as usize;
                    let prev = out[i + 3] as f32 / 255.0;
                    let na = a + prev * (1.0 - a);
                    if na > 0.0 {
                        out[i..i + 3].copy_from_slice(&rgb);
                        out[i + 3] = (na * 255.0).round() as u8;
                    }
                }
            }
            pen += m.advance_width;
        }
    }
    out
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
        let b = picture("ve.bars", &[], 700, 300).unwrap();
        assert_eq!(px(&b, 50, 10), [191, 191, 191, 255], "75% white");
        assert_eq!(px(&b, 150, 10), [191, 191, 0, 255], "yellow");
        let m = picture("ve.color", &[("color".into(), Value::Color([1.0, 0.0, 0.0, 1.0]))], 8, 8).unwrap();
        assert_eq!(px(&m, 3, 3), [255, 0, 0, 255]);
        let t = picture("ve.title", &[("text".into(), Value::Text("IIII".into())), ("size".into(), Value::Float(64.0))], 400, 200).unwrap();
        let FrameData::Cpu { planes, .. } = &t.data else { panic!() };
        let inked = planes[0].chunks(4).filter(|p| p[3] > 128).count();
        assert!(inked > 200, "the text is drawn ({inked} px)");
        assert_eq!(px(&t, 5, 5)[3], 0, "transparent around it");
        // Cached: the same parameters give the same frame.
        assert!(Arc::ptr_eq(&b, &picture("ve.bars", &[], 700, 300).unwrap()));
        assert!(picture("com.example.unknown", &[], 4, 4).is_none());
    }
}
