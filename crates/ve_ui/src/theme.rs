//! Ported from the libgui_cut mock-up.
//!
//! The look a video editor wears: near-black chrome so the picture is the
//! brightest thing on screen, a blue accent, and small dense type.
//!
//! As with every libgui theme, this is [`Theme`] with different numbers — the
//! app owns its whole visual identity.

use libgui::*;

/// Clip and timeline colours, which are the app's own vocabulary rather than
/// anything the library knows about.
pub struct Reel {
    pub ruler_bg: Color,
    pub track_bg: Color,
    pub track_bg_alt: Color,
    pub track_head: Color,
    pub grid: Color,
    pub video_fill: Color,
    pub video_head: Color,
    pub audio_fill: Color,
    pub audio_head: Color,
    pub title_fill: Color,
    pub title_head: Color,
    pub wave: Color,
    pub clip_text: Color,
    pub clip_border: Color,
    pub selected: Color,
    pub playhead: Color,
    pub in_out: Color,
    pub timecode: Color,
    pub meter_lo: Color,
    pub meter_hi: Color,
}

pub const REEL: Reel = Reel {
    ruler_bg: Color::hex(0x1b1b1b),
    track_bg: Color::hex(0x1f1f1f),
    track_bg_alt: Color::hex(0x232323),
    track_head: Color::hex(0x2b2b2b),
    grid: Color::hex(0x333333),
    video_fill: Color::hex(0x2d6e9e),
    video_head: Color::hex(0x1e4f74),
    audio_fill: Color::hex(0x2a6079),
    audio_head: Color::hex(0x1d4557),
    title_fill: Color::hex(0xa05fc8),
    title_head: Color::hex(0x7a3fa8),
    wave: Color::hex(0x7fc6e8),
    clip_text: Color::hex(0xdfe9f2),
    clip_border: Color::hex(0x14293a),
    selected: Color::hex(0xffffff),
    playhead: Color::hex(0x3a9ae8),
    in_out: Color::hex(0xe0c341),
    timecode: Color::hex(0x4aa3e8),
    meter_lo: Color::hex(0x4caf50),
    meter_hi: Color::hex(0xd94c4c),
};

pub fn theme() -> Theme {
    let mut t = Theme::dark();
    t.name = "Cut".into();

    let p = &mut t.palette;
    p.bg_app = Color::hex(0x191919);
    p.bg_panel = Color::hex(0x232323);
    p.bg_inset = Color::hex(0x141414);
    p.surface = Color::hex(0x333333);
    p.surface_hover = Color::hex(0x3d3d3d);
    p.surface_active = Color::hex(0x2a2a2a);
    p.border = Color::hex(0x101010);
    p.border_strong = Color::hex(0x454545);
    p.accent = Color::hex(0x2d8ceb);
    p.accent_hover = Color::hex(0x459ef0);
    p.accent_active = Color::hex(0x1f6fc0);
    p.focus_ring = Color::hex(0x2d8ceb);
    p.text = Color::hex(0xd6d6d6);
    p.text_muted = Color::hex(0x9d9d9d);
    p.text_faint = Color::hex(0x6f6f6f);
    p.text_on_accent = Color::hex(0xffffff);
    p.shadow = Color::rgba(0.0, 0.0, 0.0, 0.5);

    let m = &mut t.metrics;
    m.font_size = 11.0;
    m.font_size_small = 10.0;
    m.font_size_heading = 12.0;
    m.radius = 2.0;
    m.radius_large = 3.0;
    m.space = 6.0;
    m.control_height = 20.0;
    m.row_height = 18.0;
    m.tab_height = 24.0;
    m.indent = 14.0;
    m.focus_ring_width = 1.0;

    let (pal, met, d) = (t.palette, t.metrics, t.density);
    let mut t = Theme::from_parts("Cut", pal, d, met);
    t.button.height = 20.0;
    t.button.radius = 2.0;
    t.button.shadow.color = Color::TRANSPARENT;
    t.button_primary.shadow.color = Color::TRANSPARENT;
    t.selectable.height = 18.0;
    t.selectable.radius = 2.0;
    t.selectable.indicator_width = 0.0;
    t.text_input.height = 20.0;
    t.text_input.radius = 2.0;
    t.panel.radius = 0.0;
    t.panel.border = Color::hex(0x101010);
    t.panel.fill = Color::hex(0x232323);
    t.tab.height = 24.0;
    t.tab.radius = 0.0;
    t.tab.accent_height = 2.0;
    t.tab.bar_fill = Color::hex(0x1c1c1c);
    t.tab.fill_active = Color::hex(0x232323);
    t.scrollbar.width = 8.0;
    t.scrollbar.width_hover = 11.0;
    t.scrollbar.rest_alpha = 0.55;
    t.slider.track_height = 3.0;
    t.slider.knob_radius = 5.5;
    t
}
