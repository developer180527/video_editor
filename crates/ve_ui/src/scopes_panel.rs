//! Lumetri Scopes: the program monitor's picture as a waveform, RGB parade,
//! vectorscope or histogram. The scope image is measured on the GPU
//! (`ve_render::scopes`); this draws it with its graticule.

use libgui::*;
use ve_render::scopes::ScopeKind;

use crate::theme::REEL;
use crate::EditorUi;

/// Graticule lines and labels.
const GRID: Color = Color { r: 0.42, g: 0.42, b: 0.42, a: 0.55 };
const LABEL: Color = Color { r: 0.62, g: 0.62, b: 0.62, a: 1.0 };

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    app.scopes_wanted = true;
    let t = ui.theme.clone();
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
    ui.container(col, Frame { fill: REEL.inset, clip: true, ..Frame::none() }, |ui| {
        let id = ui.make_id("scope");
        let tex = app.scope_tex;
        let kind = app.view.scope;
        let size = t.metrics.font_size_small;
        let labels: Vec<FrameText> = match kind {
            ScopeKind::Waveform | ScopeKind::Parade => (0..=10).map(|i| ui.frame_text(&(i * 10).to_string())).collect(),
            ScopeKind::Histogram => [0, 64, 128, 192, 255].iter().map(|v| ui.frame_text(&v.to_string())).collect(),
            ScopeKind::Vectorscope => ["R", "Mg", "B", "Cy", "G", "Yl"].iter().map(|v| ui.frame_text(v)).collect(),
        };
        let codes: Vec<FrameText> = [0, 26, 51, 77, 102, 128, 153, 179, 204, 230, 255].iter().map(|v| ui.frame_text(&v.to_string())).collect();
        let name = ui.frame_text(kind.name());
        ui.add_leaf(id, Layout::leaf(Size::Grow(1.0), Size::Grow(1.0)), Vec2::ZERO, false, move |p, r| {
            p.rect(r, Color::BLACK, 0.0);
            p.text_left(Rect::new(r.x + 46.0, r.y + 4.0, 200.0, 14.0), size, LABEL, name);
            match kind {
                ScopeKind::Waveform | ScopeKind::Parade => {
                    let area = Rect::new(r.x + 40.0, r.y + 22.0, (r.w - 80.0).max(10.0), (r.h - 34.0).max(10.0));
                    if let Some(tex) = tex {
                        p.image(area, tex, 0.0);
                    }
                    for i in 0..=10 {
                        let y = (area.bottom() - area.h * i as f32 / 10.0).round();
                        p.rect(Rect::new(area.x, y, area.w, 1.0), GRID, 0.0);
                        p.text_right(Rect::new(r.x, y - 7.0, 34.0, 14.0), size, LABEL, labels[i]);
                        p.text_left(Rect::new(area.right() + 6.0, y - 7.0, 34.0, 14.0), size, LABEL, codes[i]);
                    }
                    if kind == ScopeKind::Parade {
                        for k in 1..3 {
                            let x = (area.x + area.w * k as f32 / 3.0).round();
                            p.rect(Rect::new(x, area.y, 1.0, area.h), GRID, 0.0);
                        }
                    }
                }
                ScopeKind::Vectorscope => {
                    let side = (r.w - 40.0).min(r.h - 34.0).max(20.0);
                    let area = Rect::new((r.center().x - side / 2.0).round(), r.y + 22.0, side, side);
                    if let Some(tex) = tex {
                        p.image(area, tex, 0.0);
                    }
                    vectorscope_graticule(p, area, &labels, size);
                }
                ScopeKind::Histogram => {
                    let area = Rect::new(r.x + 20.0, r.y + 22.0, (r.w - 40.0).max(10.0), (r.h - 44.0).max(10.0));
                    if let Some(tex) = tex {
                        p.image(area, tex, 0.0);
                    }
                    for (i, v) in [0.0, 64.0, 128.0, 192.0, 255.0].iter().enumerate() {
                        let x = (area.x + area.w * v / 255.0).round();
                        p.rect(Rect::new(x, area.y, 1.0, area.h), GRID, 0.0);
                        p.text_centered(Rect::new(x - 20.0, area.bottom() + 3.0, 40.0, 14.0), size, LABEL, labels[i]);
                    }
                }
            }
        });
        // The footer: which scope, and what it measures.
        let row = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(28.0)).padding(Insets::xy(8.0, 0.0)).gap(8.0).align(Align::Start, Align::Center);
        ui.container(row, Frame { fill: REEL.chrome, ..Frame::none() }, |ui| {
            ui.text_with("Rec. 709 · 8-bit", t.metrics.font_size_small, t.palette.text_muted);
            ui.flex();
            let names: Vec<&str> = ScopeKind::ALL.iter().map(|k| k.name()).collect();
            let mut i = ScopeKind::ALL.iter().position(|k| *k == app.view.scope).unwrap_or(0);
            ui.container(Layout::row().width(Size::Fixed(170.0)).height(Size::Fixed(20.0)), Frame::none(), |ui| {
                ui.combo_keyed("scope-kind", &mut i, &names);
            });
            app.view.scope = ScopeKind::ALL[i];
        });
    });
}

/// The vectorscope's circle, the 75% colour-bar targets and the skin tone
/// line, over a square `area` whose centre is zero chroma.
fn vectorscope_graticule(p: &mut Painter, area: Rect, labels: &[FrameText], size: f32) {
    let c = area.center();
    let radius = area.w / 2.0;
    let ring = |p: &mut Painter, r: f32, color: Color| {
        let n = 96;
        for i in 0..n {
            let (a0, a1) = (i as f32 / n as f32 * std::f32::consts::TAU, (i + 1) as f32 / n as f32 * std::f32::consts::TAU);
            p.line(Vec2::new(c.x + r * a0.cos(), c.y + r * a0.sin()), Vec2::new(c.x + r * a1.cos(), c.y + r * a1.sin()), 1.0, color);
        }
    };
    ring(p, radius - 1.0, GRID);
    p.line(Vec2::new(area.x, c.y), Vec2::new(area.right(), c.y), 1.0, GRID);
    p.line(Vec2::new(c.x, area.y), Vec2::new(c.x, area.bottom()), 1.0, GRID);
    // Where 75% bars land: the scope's mapping (Cb right, Cr up, ±0.5 to
    // the edges) applied to each bar colour.
    let at = |rgb: [f32; 3]| {
        let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        let cb = (rgb[2] - y) / 1.8556;
        let cr = (rgb[0] - y) / 1.5748;
        Vec2::new(area.x + area.w * (cb + 0.5), area.y + area.h * (0.5 - cr))
    };
    let bars = [[0.75, 0.0, 0.0], [0.75, 0.0, 0.75], [0.0, 0.0, 0.75], [0.0, 0.75, 0.75], [0.0, 0.75, 0.0], [0.75, 0.75, 0.0]];
    for (i, rgb) in bars.iter().enumerate() {
        let q = at(*rgb);
        p.rect_bordered(Rect::new(q.x - 5.0, q.y - 5.0, 10.0, 10.0), Color::TRANSPARENT, 1.0, 1.0, Color::rgba(rgb[0] + 0.25, rgb[1] + 0.25, rgb[2] + 0.25, 0.9));
        let out = Vec2::new(q.x - c.x, q.y - c.y);
        let len = (out.x * out.x + out.y * out.y).sqrt().max(1.0);
        let lp = Vec2::new(q.x + out.x / len * 14.0, q.y + out.y / len * 14.0);
        p.text_centered(Rect::new(lp.x - 12.0, lp.y - 7.0, 24.0, 14.0), size, LABEL, labels[i]);
    }
    // Skin tones fall along this line, whatever the complexion.
    let skin = at([0.75, 0.55, 0.42]);
    let d = Vec2::new(skin.x - c.x, skin.y - c.y);
    let len = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
    p.line(c, Vec2::new(c.x + d.x / len * radius, c.y + d.y / len * radius), 1.0, Color::rgba(0.85, 0.65, 0.45, 0.6));
}
