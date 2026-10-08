//! Composition root for desktop: pick an implementation of every port, build
//! the engine, hand it to the shell. Nothing else lives here.

use std::sync::Arc;
use ve_engine::Engine;

fn main() {
    let media = Arc::new(media_ffmpeg::Ffmpeg::new());
    let mut engine = Engine::new(platform_desktop::platform(media, ve_builtins::linked()));
    engine.new_project("Untitled");
    // A project named on the command line is opened; media is imported.
    for path in std::env::args().skip(1) {
        let r = if path.ends_with(".veproj") {
            let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone().into());
            engine.open(ve_model::MediaRef(format!("file:{}", abs.display()))).map_err(|e| e.to_string())
        } else {
            engine.import(&path).map(|_| ()).map_err(|e| e.to_string())
        };
        if let Err(e) = r {
            eprintln!("{path}: {e}");
        }
    }
    editor_app::run(engine, false);
}
