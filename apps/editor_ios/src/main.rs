//! Composition root for iPadOS: pick an implementation of every port, build
//! the engine, hand it to the shell. Nothing else lives here.

use std::sync::Arc;
use ve_engine::Engine;

fn main() {
    let media = Arc::new(media_ffmpeg::Ffmpeg::new());
    let mut engine = Engine::new(platform_ios::platform(media, ve_builtins::linked()));
    engine.new_project("Untitled");
    editor_app::run(engine, true);
}
