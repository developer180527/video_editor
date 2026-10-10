//! Icons as vector masks. Each icon's SVG outlines ([`crate::icon_shapes`])
//! are turned once into a filled path; libgui rasterises it at the size it
//! is shown (cached) and draws it in any colour — white on the dark theme,
//! black on a light one, the accent when lit — from one shape.
//!
//! Strokes become fills: every segment a quad and every vertex a disc
//! (round caps and joins), all wound the same way, so the non-zero rule
//! makes them one outline.

use std::collections::HashMap;
use std::sync::OnceLock;

use libgui::{Path, Vec2};

use crate::widgets::Icon;

/// The grid icons are drawn on, and their stroke width on it.
const GRID: f32 = 24.0;
const STROKE: f32 = 2.0;

/// `icon` as a filled path on the 24-unit grid.
pub(crate) fn path(icon: Icon) -> &'static Path {
    static CACHE: OnceLock<HashMap<Icon, Path>> = OnceLock::new();
    let all = CACHE.get_or_init(|| Icon::ALL.iter().map(|&i| (i, build(i))).collect());
    &all[&icon]
}

fn build(icon: Icon) -> Path {
    let (stroked, filled) = crate::icon_shapes::shapes(icon);
    let mut contours: Vec<Vec<Vec2>> = Vec::new();
    for d in filled {
        for (pts, _) in parse(d) {
            // Filled shapes are drawn with their outline too, as the set does.
            contours.push(positive(pts.clone()));
            stroke(&pts, true, &mut contours);
        }
    }
    for d in stroked {
        for (pts, closed) in parse(d) {
            stroke(&pts, closed, &mut contours);
        }
    }
    let mut path = Path::new(GRID, GRID);
    for c in contours.iter().filter(|c| c.len() >= 3) {
        path = path.move_to(c[0]);
        for &p in &c[1..] {
            path = path.line_to(p);
        }
        path = path.close();
    }
    path
}

/// Twice the signed area: positive for the winding every part here uses.
fn area(pts: &[Vec2]) -> f32 {
    (0..pts.len()).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        a.x * b.y - b.x * a.y
    }).sum()
}

fn positive(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    if area(&pts) < 0.0 {
        pts.reverse();
    }
    pts
}

/// A polyline's stroke, as quads and discs.
fn stroke(pts: &[Vec2], closed: bool, out: &mut Vec<Vec<Vec2>>) {
    let h = STROKE * 0.5;
    let n = pts.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let d = b - a;
        let len = (d.x * d.x + d.y * d.y).sqrt();
        if len < 1e-4 {
            continue;
        }
        let nrm = Vec2::new(-d.y / len * h, d.x / len * h);
        out.push(positive(vec![a - nrm, b - nrm, b + nrm, a + nrm]));
    }
    for &p in pts {
        out.push((0..16).map(|k| {
            let t = k as f32 / 16.0 * std::f32::consts::TAU;
            Vec2::new(p.x + h * t.cos(), p.y + h * t.sin())
        }).collect());
    }
}

/// SVG path data, flattened: each subpath's points and whether it closes.
/// Handles M L H V C S Q A Z, absolute and relative.
fn parse(d: &str) -> Vec<(Vec<Vec2>, bool)> {
    let mut tokens = Tokens { s: d.as_bytes(), i: 0 };
    let mut out: Vec<(Vec<Vec2>, bool)> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    let (mut pos, mut start) = (Vec2::ZERO, Vec2::ZERO);
    // The last cubic control point, for S.
    let mut last_ctrl: Option<Vec2> = None;
    let mut cmd = b'M';
    let finish = |cur: &mut Vec<Vec2>, out: &mut Vec<(Vec<Vec2>, bool)>, closed: bool| {
        if cur.len() > 1 || (cur.len() == 1 && !closed) {
            out.push((std::mem::take(cur), closed));
        }
        cur.clear();
    };
    while let Some(c) = tokens.command_or_number() {
        if let Some(c) = c {
            cmd = c;
            if matches!(c, b'Z' | b'z') {
                finish(&mut cur, &mut out, true);
                pos = start;
                last_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { pos } else { Vec2::ZERO };
        let pt = |t: &mut Tokens| -> Option<Vec2> { Some(Vec2::new(t.number()?, t.number()?) + base) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let Some(p) = pt(&mut tokens) else { break };
                finish(&mut cur, &mut out, false);
                pos = p;
                start = p;
                cur.push(p);
                // Further pairs after a move are lines.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                let Some(p) = pt(&mut tokens) else { break };
                pos = p;
                cur.push(p);
                last_ctrl = None;
            }
            b'H' => {
                let Some(x) = tokens.number() else { break };
                pos = Vec2::new(if rel { pos.x + x } else { x }, pos.y);
                cur.push(pos);
                last_ctrl = None;
            }
            b'V' => {
                let Some(y) = tokens.number() else { break };
                pos = Vec2::new(pos.x, if rel { pos.y + y } else { y });
                cur.push(pos);
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    let Some(c1) = pt(&mut tokens) else { break };
                    c1
                } else {
                    last_ctrl.map_or(pos, |l| pos + (pos - l))
                };
                let (Some(c2), Some(p)) = (pt(&mut tokens), pt(&mut tokens)) else { break };
                for k in 1..=12 {
                    let t = k as f32 / 12.0;
                    let u = 1.0 - t;
                    cur.push(pos * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + p * (t * t * t));
                }
                last_ctrl = Some(c2);
                pos = p;
            }
            b'Q' => {
                let (Some(c1), Some(p)) = (pt(&mut tokens), pt(&mut tokens)) else { break };
                for k in 1..=10 {
                    let t = k as f32 / 10.0;
                    let u = 1.0 - t;
                    cur.push(pos * (u * u) + c1 * (2.0 * u * t) + p * (t * t));
                }
                last_ctrl = None;
                pos = p;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot), Some(large), Some(sweep)) = (tokens.number(), tokens.number(), tokens.number(), tokens.flag(), tokens.flag()) else { break };
                let Some(p) = pt(&mut tokens) else { break };
                arc(pos, p, rx, ry, rot, large, sweep, &mut cur);
                last_ctrl = None;
                pos = p;
            }
            _ => break,
        }
    }
    finish(&mut cur, &mut out, false);
    out
}

/// An SVG elliptical arc from `a` to `b`, flattened into `out` (endpoint
/// to centre form, SVG 1.1 appendix F.6).
#[allow(clippy::too_many_arguments)]
fn arc(a: Vec2, b: Vec2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, out: &mut Vec<Vec2>) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || (a.x == b.x && a.y == b.y) {
        out.push(b);
        return;
    }
    let phi = rot_deg.to_radians();
    let (cs, sn) = (phi.cos(), phi.sin());
    let dx = (a.x - b.x) * 0.5;
    let dy = (a.y - b.y) * 0.5;
    let x1 = cs * dx + sn * dy;
    let y1 = -sn * dx + cs * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = (num / den.max(1e-12)).sqrt();
    if large == sweep {
        k = -k;
    }
    let cx1 = k * rx * y1 / ry;
    let cy1 = -k * ry * x1 / rx;
    let cx = cs * cx1 - sn * cy1 + (a.x + b.x) * 0.5;
    let cy = sn * cx1 + cs * cy1 + (a.y + b.y) * 0.5;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let d = (ux * vx + uy * vy) / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt());
        let s = if ux * vy - uy * vx < 0.0 { -1.0 } else { 1.0 };
        s * d.clamp(-1.0, 1.0).acos()
    };
    let t1 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let tau = std::f32::consts::TAU;
    if !sweep && dt > 0.0 {
        dt -= tau;
    } else if sweep && dt < 0.0 {
        dt += tau;
    }
    let steps = ((dt.abs() / tau * 32.0).ceil() as usize).max(2);
    for s in 1..=steps {
        let t = t1 + dt * s as f32 / steps as f32;
        let (x, y) = (rx * t.cos(), ry * t.sin());
        out.push(Vec2::new(cs * x - sn * y + cx, sn * x + cs * y + cy));
    }
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    /// The next command letter (`Some(Some(c))`), or `Some(None)` when a
    /// number follows (the previous command repeats); `None` at the end.
    fn command_or_number(&mut self) -> Option<Option<u8>> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(Some(c))
        } else {
            Some(None)
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let s = self.s;
        if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
            self.i += 1;
        }
        let mut dot = false;
        while self.i < s.len() {
            match s[self.i] {
                b'0'..=b'9' => {}
                // A second dot starts the next number: "0.5.5" is 0.5, .5.
                b'.' if !dot => dot = true,
                b'e' | b'E' if self.i + 1 < s.len() && (s[self.i + 1].is_ascii_digit() || s[self.i + 1] == b'-') => self.i += 1,
                _ => break,
            }
            self.i += 1;
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }

    /// An arc flag: one digit, which may run into the next number ("011 5").
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_svg_numbers_parse() {
        // Lucide writes "a2 2 0 0 0-2 2v.18" and "1-1.73l-.43.25".
        let p = parse("M12.22 2h-.44a2 2 0 0 0-2 2v.18l-.43.25");
        assert_eq!(p.len(), 1);
        let last = *p[0].0.last().unwrap();
        assert!((last.x - 9.35).abs() < 1e-3 && (last.y - 4.43).abs() < 1e-3, "{last:?}");
    }

    #[test]
    fn a_full_circle_comes_back_round() {
        let p = parse("M9 12A3 3 0 1 0 15 12A3 3 0 1 0 9 12Z");
        let pts = &p[0].0;
        assert!(p[0].1, "closed");
        for q in pts {
            let r = ((q.x - 12.0).powi(2) + (q.y - 12.0).powi(2)).sqrt();
            assert!((r - 3.0).abs() < 1e-3, "{q:?}");
        }
    }

    #[test]
    fn every_icon_has_a_shape() {
        for &i in Icon::ALL {
            assert!(!path(i).is_empty(), "{i:?}");
        }
    }

    /// Every icon, white on the dark theme and black on a light one, at 32
    /// and 16 px: written to `target/ui-look/icons.png` to be looked at.
    #[test]
    fn icon_sheet() {
        use libgui::*;
        let font = include_bytes!("../../../third_party/libgui/assets/Inter.ttf");
        let mut ui = Ui::new(crate::theme(), font).unwrap();
        let n = Icon::ALL.len() as f32;
        let (w, h) = (n * 40.0 + 20.0, 140.0);
        ui.begin_frame(FrameInfo { screen_size: Vec2::new(w, h), scale: 1.0, dt: 1.0 / 60.0 });
        let id = ui.make_id("sheet");
        ui.add_leaf(id, Layout::leaf(Size::Fixed(w), Size::Fixed(h)), Vec2::ZERO, false, move |p, r| {
            p.rect(Rect::new(r.x, r.y, r.w, 70.0), Color::hex(0x1a1a1a), 0.0);
            p.rect(Rect::new(r.x, r.y + 70.0, r.w, 70.0), Color::hex(0xf2f2f2), 0.0);
            for (k, &icon) in Icon::ALL.iter().enumerate() {
                let x = r.x + 10.0 + k as f32 * 40.0;
                for (row, c) in [(0.0, Color::WHITE), (70.0, Color::BLACK)] {
                    crate::widgets::draw_icon(p, Rect::new(x, r.y + row + 6.0, 32.0, 32.0), icon, c);
                    crate::widgets::draw_icon(p, Rect::new(x + 8.0, r.y + row + 46.0, 16.0, 16.0), icon, c);
                }
            }
        });
        let out = ui.end_frame();
        let img = libgui_soft::SoftRenderer::new().render_to_image(&out, w as u32, h as u32);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-look");
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("icons.png")).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&img.data).unwrap();
    }
}
