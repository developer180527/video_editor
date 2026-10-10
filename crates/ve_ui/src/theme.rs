//! The look a video editor wears, from `theme.toml` (dark: neutral
//! near-black chrome so the picture is the brightest thing on screen) or
//! `theme_light.toml` (light greys, grey edges), with a blue accent and
//! small dense type. The Settings panel picks one, or follows the system.
//!
//! The file is a libgui theme plus a `[reel]` table of the editor's own
//! colours — the timeline, clips and the neutral tones the panels are drawn
//! in — so the whole look changes from one place. It is compiled in; a typo
//! in it fails the app's tests, not a user's launch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use libgui::*;

/// The theme files, compiled in.
const THEME_TOML: &str = include_str!("../theme.toml");
const LIGHT_TOML: &str = include_str!("../theme_light.toml");

/// Which of the two looks is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

static DARK: LazyLock<(Theme, Reel)> = LazyLock::new(|| parse(THEME_TOML).expect("theme.toml"));
static LIGHT: LazyLock<(Theme, Reel)> = LazyLock::new(|| parse(LIGHT_TOML).expect("theme_light.toml"));
static SHOWING_LIGHT: AtomicBool = AtomicBool::new(false);

/// The editor's own colours (`[reel]` in `theme.toml`).
pub struct Reel {
    // Neutral tones, darkest to lightest.
    pub line: Color,
    pub inset: Color,
    pub chrome_deep: Color,
    pub chrome: Color,
    pub panel: Color,
    pub raised: Color,
    pub raised_hi: Color,
    pub tick: Color,
    pub label: Color,
    pub text_soft: Color,
    pub bright: Color,
    // Timeline.
    pub ruler_bg: Color,
    pub track_bg: Color,
    pub track_bg_alt: Color,
    pub track_head: Color,
    pub grid: Color,
    pub playhead: Color,
    pub in_out: Color,
    pub in_out_range: Color,
    pub timecode: Color,
    pub selected: Color,
    // Clips.
    pub video_fill: Color,
    pub video_head: Color,
    pub audio_fill: Color,
    pub audio_head: Color,
    pub title_fill: Color,
    pub title_head: Color,
    pub nest_fill: Color,
    pub nest_head: Color,
    pub wave: Color,
    pub clip_text: Color,
    pub clip_border: Color,
    pub transition: Color,
    pub transition_text: Color,
    pub badge: Color,
    // Meters.
    pub meter_lo: Color,
    pub meter_mid: Color,
    pub meter_hi: Color,
}

/// The editor's colours of the look that is showing.
pub static REEL: ReelRef = ReelRef;

/// Reads as the showing look's [`Reel`] (`REEL.panel`, …).
pub struct ReelRef;

impl std::ops::Deref for ReelRef {
    type Target = Reel;
    fn deref(&self) -> &Reel {
        if SHOWING_LIGHT.load(Ordering::Relaxed) {
            &LIGHT.1
        } else {
            &DARK.1
        }
    }
}

/// The dark libgui theme (what a window starts with).
pub fn theme() -> Theme {
    DARK.0.clone()
}

/// The libgui theme for `a`.
pub fn theme_for(a: Appearance) -> Theme {
    match a {
        Appearance::Dark => DARK.0.clone(),
        Appearance::Light => LIGHT.0.clone(),
    }
}

/// Show `a`: [`REEL`] reads its colours from now on.
pub(crate) fn set_appearance(a: Appearance) {
    SHOWING_LIGHT.store(a == Appearance::Light, Ordering::Relaxed);
}

/// Split a theme file into its libgui theme and the `[reel]` colours.
fn parse(src: &str) -> Result<(Theme, Reel), String> {
    let mut doc: toml::Table = src.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let reel = doc.remove("reel").and_then(|v| v.as_table().cloned()).ok_or("theme.toml has no [reel] table")?;
    let mut theme = Theme::from_toml(&toml::to_string(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // Not part of libgui's theme file: follow the accent, keep the sizes.
    theme.palette.focus_ring = theme.palette.accent;
    theme.metrics.indent = 14.0;
    theme.metrics.focus_ring_width = 1.0;
    let c = |k: &str| -> Result<Color, String> {
        let v = reel.get(k).and_then(|v| v.as_str()).ok_or(format!("[reel] is missing `{k}`"))?;
        Color::parse_hex(v).ok_or(format!("[reel] `{k}` = `{v}` is not a colour"))
    };
    let r = Reel {
        line: c("line")?,
        inset: c("inset")?,
        chrome_deep: c("chrome_deep")?,
        chrome: c("chrome")?,
        panel: c("panel")?,
        raised: c("raised")?,
        raised_hi: c("raised_hi")?,
        tick: c("tick")?,
        label: c("label")?,
        text_soft: c("text_soft")?,
        bright: c("bright")?,
        ruler_bg: c("ruler_bg")?,
        track_bg: c("track_bg")?,
        track_bg_alt: c("track_bg_alt")?,
        track_head: c("track_head")?,
        grid: c("grid")?,
        playhead: c("playhead")?,
        in_out: c("in_out")?,
        in_out_range: c("in_out_range")?,
        timecode: c("timecode")?,
        selected: c("selected")?,
        video_fill: c("video_fill")?,
        video_head: c("video_head")?,
        audio_fill: c("audio_fill")?,
        audio_head: c("audio_head")?,
        title_fill: c("title_fill")?,
        title_head: c("title_head")?,
        nest_fill: c("nest_fill")?,
        nest_head: c("nest_head")?,
        wave: c("wave")?,
        clip_text: c("clip_text")?,
        clip_border: c("clip_border")?,
        transition: c("transition")?,
        transition_text: c("transition_text")?,
        badge: c("badge")?,
        meter_lo: c("meter_lo")?,
        meter_mid: c("meter_mid")?,
        meter_hi: c("meter_hi")?,
    };
    let known = [
        "line", "inset", "chrome_deep", "chrome", "panel", "raised", "raised_hi", "tick", "label", "text_soft", "bright", "ruler_bg", "track_bg", "track_bg_alt",
        "track_head", "grid", "playhead", "in_out", "in_out_range", "timecode", "selected", "video_fill", "video_head", "audio_fill", "audio_head", "title_fill",
        "title_head", "nest_fill", "nest_head", "wave", "clip_text", "clip_border", "transition", "transition_text", "badge", "meter_lo", "meter_mid", "meter_hi",
    ];
    if let Some(k) = reel.keys().find(|k| !known.contains(&k.as_str())) {
        return Err(format!("[reel] has an unknown key `{k}`"));
    }
    Ok((theme, r))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_light_theme_parses_and_has_grey_edges() {
        let (t, r) = super::parse(super::LIGHT_TOML).unwrap();
        assert_eq!(t.name, "Cut Light");
        assert!(t.palette.bg_panel.r > 0.8 && t.palette.text.r < 0.2);
        // Edges are grey: neither black nor white, and neutral.
        for c in [t.palette.border, t.palette.border_strong, r.line, r.grid] {
            assert!(c.r > 0.3 && c.r < 0.85, "{c:?}");
            assert!((c.r - c.g).abs() < 1e-6 && (c.g - c.b).abs() < 1e-6, "neutral: {c:?}");
        }
    }

    #[test]
    fn theme_toml_parses() {
        let (t, r) = super::parse(super::THEME_TOML).unwrap();
        assert_eq!(t.name, "Cut");
        // Near-black, never pure black, and neutral (no colour cast).
        for c in [t.palette.bg_app, t.palette.bg_panel, t.palette.bg_inset, r.chrome, r.track_bg, r.line] {
            assert!(c.r > 0.0 && c.r < 0.1, "{c:?}");
            assert!((c.r - c.g).abs() < 1e-6 && (c.g - c.b).abs() < 1e-6, "neutral: {c:?}");
        }
    }
}
