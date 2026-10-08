//! Raw bindings, generated at build time from the pinned FFmpeg's headers.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, unnecessary_transmutes, clippy::all)]
include!(concat!(env!("OUT_DIR"), "/ffmpeg.rs"));
