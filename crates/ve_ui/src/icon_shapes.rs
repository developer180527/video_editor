//! Icon outlines as SVG path data on a 24-unit grid: `(stroked, filled)`.
//! Strokes are 2 units wide with round ends and joins.
//!
//! Most come from Lucide 0.460.0 (https://lucide.dev), under the ISC licence:
//!
//! > Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as
//! > part of Feather (MIT). All other copyright (c) for Lucide are held by
//! > Lucide Contributors 2022.
//! >
//! > Permission to use, copy, modify, and/or distribute this software for any
//! > purpose with or without fee is hereby granted, provided that the above
//! > copyright notice and this permission notice appear in all copies.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
//! > WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
//! > MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
//! > SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
//! > WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
//! > ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR
//! > IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
//!
//! The transport icons are Lucide's, filled; the editing-tool icons (marks,
//! marker, insert, overwrite, track select, ripple, rolling, slip) are drawn
//! for this app on the same grid.

use crate::widgets::Icon;

type Shapes = (&'static [&'static str], &'static [&'static str]);

pub(crate) fn shapes(icon: Icon) -> Shapes {
    match icon {
        Icon::Home => (&["M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8", "M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"], &[]),
        Icon::Share => (&["M15 5A3 3 0 1 0 21 5A3 3 0 1 0 15 5Z", "M3 12A3 3 0 1 0 9 12A3 3 0 1 0 3 12Z", "M15 19A3 3 0 1 0 21 19A3 3 0 1 0 15 19Z", "M8.59 13.51L15.42 17.49", "M15.41 6.51L8.59 10.49"], &[]),
        Icon::Menu => (&["M4 12L20 12", "M4 6L20 6", "M4 18L20 18"], &[]),
        Icon::Expand => (&["M15 3 L21 3 L21 9", "M9 21 L3 21 L3 15", "M21 3L14 10", "M3 21L10 14"], &[]),
        Icon::Panel => (&["M5 3H19A2 2 0 0 1 21 5V19A2 2 0 0 1 19 21H5A2 2 0 0 1 3 19V5A2 2 0 0 1 5 3Z", "M9 3v18"], &[]),
        Icon::Play => (&[], &["M6 3 L20 12 L6 21 L6 3Z"]),
        Icon::Pause => (&[], &["M15 4H17A1 1 0 0 1 18 5V19A1 1 0 0 1 17 20H15A1 1 0 0 1 14 19V5A1 1 0 0 1 15 4Z", "M7 4H9A1 1 0 0 1 10 5V19A1 1 0 0 1 9 20H7A1 1 0 0 1 6 19V5A1 1 0 0 1 7 4Z"]),
        Icon::StepBack => (&["M18 20L18 4"], &["M14 20 L4 12 L14 4Z"]),
        Icon::StepForward => (&["M6 4L6 20"], &["M10 4 L20 12 L10 20Z"]),
        Icon::JumpStart => (&["M5 19L5 5"], &["M19 20 L9 12 L19 4 L19 20Z"]),
        Icon::JumpEnd => (&["M19 5L19 19"], &["M5 4 L15 12 L5 20 L5 4Z"]),
        Icon::MarkIn => (&["M15 4H9v16h6"], &[]),
        Icon::MarkOut => (&["M9 4h6v16H9"], &[]),
        Icon::Marker => (&[], &["M6 3h12v11l-6 6-6-6Z"]),
        Icon::Insert => (&["M12 2v9", "M8.5 7.5 12 11l3.5-3.5", "M9 15H3v5h6", "M15 15h6v5h-6"], &[]),
        Icon::Overwrite => (&["M12 2v8", "M8.5 6.5 12 10l3.5-3.5", "M3 14h18v6H3Z"], &[]),
        Icon::Camera => (&["M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z", "M9 13A3 3 0 1 0 15 13A3 3 0 1 0 9 13Z"], &[]),
        Icon::Export => (&["M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4", "M17 8 L12 3 L7 8", "M12 3L12 15"], &[]),
        Icon::Settings => (&["M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z", "M9 12A3 3 0 1 0 15 12A3 3 0 1 0 9 12Z"], &[]),
        Icon::Wrench => (&["M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"], &[]),
        Icon::Search => (&["M3 11A8 8 0 1 0 19 11A8 8 0 1 0 3 11Z", "m21 21-4.3-4.3"], &[]),
        Icon::Folder => (&["M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"], &[]),
        Icon::List => (&["M3 12h.01", "M3 18h.01", "M3 6h.01", "M8 12h13", "M8 18h13", "M8 6h13"], &[]),
        Icon::Grid => (&["M4 3H9A1 1 0 0 1 10 4V9A1 1 0 0 1 9 10H4A1 1 0 0 1 3 9V4A1 1 0 0 1 4 3Z", "M15 3H20A1 1 0 0 1 21 4V9A1 1 0 0 1 20 10H15A1 1 0 0 1 14 9V4A1 1 0 0 1 15 3Z", "M15 14H20A1 1 0 0 1 21 15V20A1 1 0 0 1 20 21H15A1 1 0 0 1 14 20V15A1 1 0 0 1 15 14Z", "M4 14H9A1 1 0 0 1 10 15V20A1 1 0 0 1 9 21H4A1 1 0 0 1 3 20V15A1 1 0 0 1 4 14Z"], &[]),
        Icon::Freeform => (&["M4 3H9A1 1 0 0 1 10 4V11A1 1 0 0 1 9 12H4A1 1 0 0 1 3 11V4A1 1 0 0 1 4 3Z", "M15 3H20A1 1 0 0 1 21 4V7A1 1 0 0 1 20 8H15A1 1 0 0 1 14 7V4A1 1 0 0 1 15 3Z", "M15 12H20A1 1 0 0 1 21 13V20A1 1 0 0 1 20 21H15A1 1 0 0 1 14 20V13A1 1 0 0 1 15 12Z", "M4 16H9A1 1 0 0 1 10 17V20A1 1 0 0 1 9 21H4A1 1 0 0 1 3 20V17A1 1 0 0 1 4 16Z"], &[]),
        Icon::Zoom => (&["M3 11A8 8 0 1 0 19 11A8 8 0 1 0 3 11Z", "M21 21L16.65 16.65", "M11 8L11 14", "M8 11L14 11"], &[]),
        Icon::NewBin => (&["M12 10v6", "M9 13h6", "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"], &[]),
        Icon::Trash => (&["M3 6h18", "M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6", "M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2", "M10 11L10 17", "M14 11L14 17"], &[]),
        Icon::Pen => (&["M15.707 21.293a1 1 0 0 1-1.414 0l-1.586-1.586a1 1 0 0 1 0-1.414l5.586-5.586a1 1 0 0 1 1.414 0l1.586 1.586a1 1 0 0 1 0 1.414z", "m18 13-1.375-6.874a1 1 0 0 0-.746-.776L3.235 2.028a1 1 0 0 0-1.207 1.207L5.35 15.879a1 1 0 0 0 .776.746L13 18", "m2.3 2.3 7.286 7.286", "M9 11A2 2 0 1 0 13 11A2 2 0 1 0 9 11Z"], &[]),
        Icon::Hand => (&["M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2", "M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2", "M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8", "M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"], &[]),
        Icon::Razor => (&["M3 6A3 3 0 1 0 9 6A3 3 0 1 0 3 6Z", "M8.12 8.12 12 12", "M20 4 8.12 15.88", "M3 18A3 3 0 1 0 9 18A3 3 0 1 0 3 18Z", "M14.8 14.8 20 20"], &[]),
        Icon::Select => (&["M4.037 4.688a.495.495 0 0 1 .651-.651l16 6.5a.5.5 0 0 1-.063.947l-6.124 1.58a2 2 0 0 0-1.438 1.435l-1.579 6.126a.5.5 0 0 1-.947.063z"], &[]),
        Icon::TrackSelect => (&["M4 4v16", "M8 12h12", "M16 8l4 4-4 4"], &[]),
        Icon::Ripple => (&["M5 4h4v16H5", "M13 12h8", "M18 9l3 3-3 3"], &[]),
        Icon::Rolling => (&["M12 4v16", "M9 12H3", "M5.5 9.5 3 12l2.5 2.5", "M15 12h6", "M18.5 9.5 21 12l-2.5 2.5"], &[]),
        Icon::Slip => (&["M8 5v14", "M16 5v14", "M2 12h20", "M4.5 9.5 2 12l2.5 2.5", "M19.5 9.5 22 12l-2.5 2.5"], &[]),
        Icon::Rect => (&["M5 3H19A2 2 0 0 1 21 5V19A2 2 0 0 1 19 21H5A2 2 0 0 1 3 19V5A2 2 0 0 1 5 3Z"], &[]),
        Icon::Type => (&["M4 7 L4 4 L20 4 L20 7", "M9 20L15 20", "M12 4L12 20"], &[]),
        Icon::Lock => (&["M5 11H19A2 2 0 0 1 21 13V20A2 2 0 0 1 19 22H5A2 2 0 0 1 3 20V13A2 2 0 0 1 5 11Z", "M7 11V7a5 5 0 0 1 10 0v4"], &[]),
        Icon::Eye => (&["M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0", "M9 12A3 3 0 1 0 15 12A3 3 0 1 0 9 12Z"], &[]),
        Icon::Mic => (&["M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z", "M19 10v2a7 7 0 0 1-14 0v-2", "M12 19L12 22"], &[]),
        Icon::Speaker => (&["M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z", "M16 9a5 5 0 0 1 0 6", "M19.364 18.364a9 9 0 0 0 0-12.728"], &[]),
        Icon::Snap => (&["m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15", "m5 8 4 4", "m12 15 4 4"], &[]),
        Icon::LinkedSelection => (&["M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71", "M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"], &[]),
        Icon::Captions => (&["M5 5H19A2 2 0 0 1 21 7V17A2 2 0 0 1 19 19H5A2 2 0 0 1 3 17V7A2 2 0 0 1 5 5Z", "M7 15h4M15 15h2M7 11h2M13 11h4"], &[]),
        Icon::Stopwatch => (&["M10 2L14 2", "M12 14L15 11", "M4 14A8 8 0 1 0 20 14A8 8 0 1 0 4 14Z"], &[]),
        Icon::Reset => (&["M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", "M3 3v5h5"], &[]),
        Icon::Chevron => (&["m6 9 6 6 6-6"], &[]),
        Icon::ChevronRight => (&["m9 18 6-6-6-6"], &[]),
        Icon::Sort => (&["m3 16 4 4 4-4", "M7 20V4", "m21 8-4-4-4 4", "M17 4v16"], &[]),
        Icon::Effects => (&["M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z", "M20 3v4", "M22 5h-4", "M4 17v2", "M5 18H3"], &[]),
        Icon::AddTrack => (&["M11 12H3", "M16 6H3", "M16 18H3", "M18 9v6", "M21 12h-6"], &[]),
    }
}
