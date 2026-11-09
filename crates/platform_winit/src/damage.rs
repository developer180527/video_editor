//! What changed on screen since the last frame, so only that is repainted.
//!
//! A frame's draw list is compared with the last one drawn: an instance in
//! one and not the other (moved, recoloured, new, gone) damages where it
//! covers, as does every instance showing a texture whose picture changed.
//! While a clip plays, that is the monitor, the playhead, the timecode and the
//! meters — not the whole window, which the GPU otherwise shades again at
//! 3–4 times its area (panels over panels) for every picture.

use libgui::{Batch, Instance, TextureId};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// The keys are hashes already: used as they are.
#[derive(Default)]
struct Same(u64);

impl Hasher for Same {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("only u64 keys")
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

/// A rectangle in physical pixels, `[x0, y0, x1, y1)`.
pub type PxRect = [u32; 4];

/// More rectangles than this are merged (each costs a pass over the draw
/// list; the merged area is shaded once).
const MAX_RECTS: usize = 8;

#[derive(Default)]
pub struct Tracker {
    /// Each instance of the last frame drawn: its content hash and where it
    /// covers (logical px).
    prev: Vec<(u64, [f32; 4])>,
    /// What else the picture depends on: physical size, scale, clear colour.
    frame: Option<(u32, u32, u32, [u32; 4])>,
    counts: HashMap<u64, u32, BuildHasherDefault<Same>>,
    /// Scratch kept between frames: last frame's instances, this frame's
    /// textures, the damage in logical px.
    old: Vec<(u64, [f32; 4])>,
    tex: Vec<TextureId>,
    hurt: Vec<[f32; 4]>,
    /// Where each instance of the frame covers, by index (for `within`).
    covers: Vec<[f32; 4]>,
    scale: f32,
}

/// What to repaint.
#[derive(Debug, PartialEq)]
pub enum Damage {
    Nothing,
    All,
    Rects(Vec<PxRect>),
}

impl Tracker {
    /// Compare a frame with the last one and remember it as drawn.
    /// `changed`: user textures whose pictures changed (`None`: unknown, so
    /// any of them may have). `size` is the target's physical size.
    #[allow(clippy::too_many_arguments)]
    pub fn diff(
        &mut self,
        instances: &[Instance],
        batches: &[Batch],
        changed: Option<&[TextureId]>,
        size: (u32, u32),
        scale: f32,
        clear: [f32; 4],
    ) -> Damage {
        let frame = Some((size.0, size.1, scale.to_bits(), clear.map(f32::to_bits)));
        // The glyph atlas is not compared: a page is emptied only when the
        // frame being built does not use it, and a repack re-rasterises all
        // the frame draws, so an instance unchanged (same page, same place
        // on it) shows the same pixels in both frames.
        let whole = self.frame != frame;
        self.frame = frame;

        // This frame's instances become `prev`; last frame's are `old`.
        std::mem::swap(&mut self.prev, &mut self.old);
        self.prev.clear();
        self.tex.clear();
        self.covers.clear();
        self.covers.resize(instances.len(), [f32::MIN, f32::MIN, f32::MAX, f32::MAX]);
        self.scale = scale;
        for b in batches {
            for i in b.range.clone() {
                if let Some(inst) = instances.get(i as usize) {
                    let c = bounds(inst);
                    self.prev.push((hash(inst, b.texture), c));
                    self.tex.push(b.texture);
                    self.covers[i as usize] = c;
                }
            }
        }
        if whole {
            return Damage::All;
        }
        self.hurt.clear();
        self.counts.clear();
        for (h, _) in &self.old {
            *self.counts.entry(*h).or_default() += 1;
        }
        for ((h, b), t) in self.prev.iter().zip(&self.tex) {
            let picture = matches!(t, TextureId::User(_)) && changed.is_none_or(|c| c.contains(t));
            match self.counts.get_mut(h) {
                Some(n) if *n > 0 && !picture => *n -= 1,
                Some(n) if *n > 0 => {
                    *n -= 1;
                    self.hurt.push(*b);
                }
                _ => self.hurt.push(*b),
            }
        }
        // What was drawn and is gone.
        for (h, b) in &self.old {
            if let Some(n) = self.counts.get_mut(h).filter(|n| **n > 0) {
                *n -= 1;
                self.hurt.push(*b);
            }
        }
        to_pixels(&self.hurt, size, scale)
    }

    /// The parts of the last frame's `batches` that can touch `r` (physical
    /// px): what a pass scissored to `r` has to draw. A few instances in
    /// between that do not are drawn along (a draw call costs more than a
    /// few quads the scissor throws away).
    pub fn within(&self, batches: &[Batch], r: PxRect, out: &mut Vec<Batch>) {
        const GAP: u32 = 16;
        out.clear();
        let k = 1.0 / self.scale.max(1e-6);
        let l = [r[0] as f32 * k, r[1] as f32 * k, r[2] as f32 * k, r[3] as f32 * k];
        for b in batches {
            let mut run: Option<(u32, u32)> = None;
            for i in b.range.clone() {
                let c = self.covers.get(i as usize).copied().unwrap_or([f32::MIN, f32::MIN, f32::MAX, f32::MAX]);
                if !(c[0] < l[2] && l[0] < c[2] && c[1] < l[3] && l[1] < c[3]) {
                    continue;
                }
                run = match run {
                    Some((s, e)) if i - e <= GAP => Some((s, i + 1)),
                    Some((s, e)) => {
                        out.push(Batch { texture: b.texture, range: s..e });
                        Some((i, i + 1))
                    }
                    None => Some((i, i + 1)),
                };
            }
            if let Some((s, e)) = run {
                out.push(Batch { texture: b.texture, range: s..e });
            }
        }
    }
}

/// Logical rectangles as merged, clamped pixel rectangles.
fn to_pixels(hurt: &[[f32; 4]], size: (u32, u32), scale: f32) -> Damage {
    let mut rects: Vec<PxRect> = Vec::new();
    for r in hurt {
        let x0 = (r[0] * scale).floor().max(0.0) as u32;
        let y0 = (r[1] * scale).floor().max(0.0) as u32;
        let x1 = ((r[2] * scale).ceil().max(0.0) as u32).min(size.0);
        let y1 = ((r[3] * scale).ceil().max(0.0) as u32).min(size.1);
        if x1 > x0 && y1 > y0 {
            rects.push([x0, y0, x1, y1]);
        }
    }
    if rects.is_empty() {
        return Damage::Nothing;
    }
    // Overlapping rectangles become one; then the pair that wastes least
    // when joined, until few enough are left.
    loop {
        let mut joined = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if overlaps(rects[i], rects[j]) {
                    rects[i] = union(rects[i], rects[j]);
                    rects.swap_remove(j);
                    joined = true;
                    break 'outer;
                }
            }
        }
        if joined {
            continue;
        }
        if rects.len() <= MAX_RECTS {
            break;
        }
        let mut best = (u64::MAX, 0, 1);
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let waste = area(union(rects[i], rects[j])) - area(rects[i]) - area(rects[j]);
                if waste < best.0 {
                    best = (waste, i, j);
                }
            }
        }
        rects[best.1] = union(rects[best.1], rects[best.2]);
        rects.swap_remove(best.2);
    }
    // Most of the window: one full pass costs the same and draws no seams.
    let total: u64 = rects.iter().map(|r| area(*r)).sum();
    if total * 10 >= size.0 as u64 * size.1 as u64 * 7 {
        return Damage::All;
    }
    Damage::Rects(rects)
}

fn overlaps(a: PxRect, b: PxRect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn union(a: PxRect, b: PxRect) -> PxRect {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn area(r: PxRect) -> u64 {
    (r[2] - r[0]) as u64 * (r[3] - r[1]) as u64
}

/// The instance's content and the texture it reads.
fn hash(i: &Instance, t: TextureId) -> u64 {
    // FNV-1a over the words: ~800 instances a frame, so speed over strength
    // (a collision only means a repaint is missed for that one frame).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |w: u32| {
        h ^= w as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for w in bytemuck::cast_slice::<Instance, u32>(std::slice::from_ref(i)) {
        eat(*w);
    }
    let (kind, n) = match t {
        TextureId::Atlas(n) => (1, n as u64),
        TextureId::User(n) => (2, n),
    };
    eat(kind);
    eat(n as u32);
    eat((n >> 32) as u32);
    h
}

/// Where an instance can touch pixels (logical px), within its clip: the
/// quad the vertex shader makes (libgui's ui.wgsl), and a pixel more for
/// anti-aliasing.
fn bounds(i: &Instance) -> [f32; 4] {
    const SHAPE: u32 = 0;
    const GLYPH: u32 = 1;
    const IMAGE: u32 = 2;
    const TRIANGLE: u32 = 4;
    let kind = i.params[3].round() as u32;
    let [x, y, w, h] = i.rect;
    let turned =
        (kind == IMAGE || kind == GLYPH) && (i.border_color[0] != 0.0 || i.border_color[1] != 0.0);
    let mut r = if kind == TRIANGLE {
        // The triangle's corners, pushed out for its fringe.
        let xs = [i.uv[0], i.uv[2], i.border_color[0]];
        let ys = [i.uv[1], i.uv[3], i.border_color[1]];
        let pad = 4.0;
        [
            xs.iter().copied().fold(f32::MAX, f32::min) - pad,
            ys.iter().copied().fold(f32::MAX, f32::min) - pad,
            xs.iter().copied().fold(f32::MIN, f32::max) + pad,
            ys.iter().copied().fold(f32::MIN, f32::max) + pad,
        ]
    } else if turned {
        // Turned about its centre: the circle it sweeps.
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let rad = (w * w + h * h).sqrt() * 0.5 + 2.0;
        [cx - rad, cy - rad, cx + rad, cy + rad]
    } else {
        let pad = if kind == SHAPE {
            i.params[2].max(0.0) + 2.0
        } else {
            2.0
        };
        [x - pad, y - pad, x + w + pad, y + h + pad]
    };
    let c = i.clip;
    r = [
        r[0].max(c[0] - 1.0),
        r[1].max(c[1] - 1.0),
        r[2].min(c[2] + 1.0),
        r[3].min(c[3] + 1.0),
    ];
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32, color: f32) -> Instance {
        Instance {
            rect: [x, y, w, h],
            uv: [0.0; 4],
            color: [color, 0.0, 0.0, 1.0],
            border_color: [0.0; 4],
            clip: [0.0, 0.0, 1000.0, 1000.0],
            params: [0.0; 4],
        }
    }

    fn atlas(n: u32) -> Batch {
        Batch {
            texture: TextureId::Atlas(0),
            range: 0..n,
        }
    }

    fn run(
        t: &mut Tracker,
        inst: &[Instance],
        batches: &[Batch],
        changed: Option<&[TextureId]>,
    ) -> Damage {
        t.diff(
            inst,
            batches,
            changed,
            (2000, 2000),
            2.0,
            [0.0, 0.0, 0.0, 1.0],
        )
    }

    #[test]
    fn first_frame_and_new_size_repaint_everything() {
        let mut t = Tracker::default();
        let a = [rect(10.0, 10.0, 20.0, 20.0, 0.5)];
        assert_eq!(run(&mut t, &a, &[atlas(1)], Some(&[])), Damage::All);
        assert_eq!(run(&mut t, &a, &[atlas(1)], Some(&[])), Damage::Nothing);
        assert_eq!(
            t.diff(
                &a,
                &[atlas(1)],
                Some(&[]),
                (1000, 1000),
                2.0,
                [0.0, 0.0, 0.0, 1.0]
            ),
            Damage::All
        );
        // A new clear colour.
        assert_eq!(
            t.diff(
                &a,
                &[atlas(1)],
                Some(&[]),
                (1000, 1000),
                2.0,
                [0.1, 0.0, 0.0, 1.0]
            ),
            Damage::All
        );
    }

    #[test]
    fn a_moved_rect_damages_both_places() {
        let mut t = Tracker::default();
        let still = rect(500.0, 500.0, 100.0, 100.0, 0.2);
        run(
            &mut t,
            &[still, rect(10.0, 10.0, 20.0, 20.0, 0.5)],
            &[atlas(2)],
            Some(&[]),
        );
        let Damage::Rects(mut d) = run(
            &mut t,
            &[still, rect(50.0, 10.0, 20.0, 20.0, 0.5)],
            &[atlas(2)],
            Some(&[]),
        ) else {
            panic!()
        };
        // Old place 8..32 and new 48..72 (logical, with the AA pad), ×2.
        d.sort();
        assert_eq!(d, vec![[16, 16, 64, 64], [96, 16, 144, 64]]);
    }

    #[test]
    fn a_picture_that_changed_damages_its_image() {
        let mut t = Tracker::default();
        let mut img = rect(100.0, 100.0, 200.0, 100.0, 1.0);
        img.params[3] = 2.0;
        let b = [Batch {
            texture: TextureId::User(7),
            range: 0..1,
        }];
        run(&mut t, &[img], &b, Some(&[]));
        assert_eq!(run(&mut t, &[img], &b, Some(&[])), Damage::Nothing);
        assert_eq!(
            run(&mut t, &[img], &b, Some(&[TextureId::User(7)])),
            Damage::Rects(vec![[196, 196, 604, 404]])
        );
        // Not told: assume it did.
        assert_eq!(
            run(&mut t, &[img], &b, None),
            Damage::Rects(vec![[196, 196, 604, 404]])
        );
    }

    #[test]
    fn clipped_parts_are_not_damage() {
        let mut t = Tracker::default();
        let mut r = rect(0.0, 0.0, 800.0, 20.0, 0.5);
        r.clip = [100.0, 0.0, 200.0, 100.0];
        run(&mut t, &[], &[], Some(&[]));
        assert_eq!(
            run(&mut t, &[r], &[atlas(1)], Some(&[])),
            Damage::Rects(vec![[198, 0, 402, 44]])
        );
    }

    #[test]
    fn a_rect_draws_only_what_reaches_it() {
        let mut t = Tracker::default();
        // Two far apart, many between that miss the rect, then one more.
        let mut inst = vec![rect(0.0, 0.0, 10.0, 10.0, 0.1)];
        inst.extend((0..40).map(|k| rect(500.0, k as f32, 5.0, 1.0, 0.2)));
        inst.push(rect(5.0, 5.0, 10.0, 10.0, 0.3));
        let b = [Batch { texture: TextureId::Atlas(0), range: 0..42 }, Batch { texture: TextureId::User(1), range: 42..42 }];
        run(&mut t, &inst, &b, Some(&[]));
        let mut out = Vec::new();
        t.within(&b, [0, 0, 40, 40], &mut out);
        let got: Vec<_> = out.iter().map(|b| b.range.clone()).collect();
        assert_eq!(got, vec![0..1, 41..42]);
        // Next to each other: one draw. Logical 500..510 × 0..10 reaches the
        // strips at y 0..11 (each padded 2 px for anti-aliasing).
        t.within(&b, [1000, 0, 1020, 20], &mut out);
        assert_eq!(out.iter().map(|b| b.range.clone()).collect::<Vec<_>>(), vec![1..13]);
    }

    #[test]
    fn many_small_changes_merge_and_a_big_one_repaints_all() {
        let mut t = Tracker::default();
        run(&mut t, &[], &[], Some(&[]));
        let dots: Vec<_> = (0..30)
            .map(|k| rect(k as f32 * 30.0, k as f32 * 30.0, 4.0, 4.0, 0.5))
            .collect();
        match run(&mut t, &dots, &[atlas(30)], Some(&[])) {
            Damage::Rects(r) => assert!(r.len() <= MAX_RECTS),
            d => panic!("{d:?}"),
        }
        assert_eq!(
            run(
                &mut t,
                &[rect(0.0, 0.0, 990.0, 990.0, 0.1)],
                &[atlas(1)],
                Some(&[])
            ),
            Damage::All
        );
    }
}
