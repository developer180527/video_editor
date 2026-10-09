//! The project file: JSON, versioned, migrated forward on load.
//!
//! ```json
//! { "format": "ve-project", "schema_version": 1, "project": { ... } }
//! ```
//! A file from a newer version is refused rather than half-read.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use thiserror::Error;
use ve_model::{validate, ModelError, Project, SCHEMA_VERSION};

pub const FILE_FORMAT: &str = "ve-project";

#[derive(Debug, Error)]
pub enum ProjectFileError {
    #[error("not a project file")]
    NotAProject,
    #[error("made by a newer version (schema {0}, this app reads up to {SCHEMA_VERSION})")]
    TooNew(u32),
    #[error("damaged project: {0}")]
    Damaged(String),
    #[error("project breaks a document rule: {0}")]
    Invalid(#[from] ModelError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Serialize)]
struct Out<'a> {
    format: &'static str,
    schema_version: u32,
    project: &'a Project,
}

#[derive(Deserialize)]
struct Header {
    format: String,
    schema_version: u32,
}

pub fn write(p: &Project, w: &mut dyn Write) -> Result<(), ProjectFileError> {
    let out = Out { format: FILE_FORMAT, schema_version: SCHEMA_VERSION, project: p };
    serde_json::to_writer_pretty(&mut *w, &out).map_err(|e| ProjectFileError::Damaged(e.to_string()))?;
    w.flush()?;
    Ok(())
}

pub fn read(r: &mut dyn Read) -> Result<Project, ProjectFileError> {
    let mut text = String::new();
    r.read_to_string(&mut text)?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| ProjectFileError::Damaged(e.to_string()))?;
    let header: Header = serde_json::from_value(value.clone()).map_err(|_| ProjectFileError::NotAProject)?;
    if header.format != FILE_FORMAT {
        return Err(ProjectFileError::NotAProject);
    }
    if header.schema_version > SCHEMA_VERSION {
        return Err(ProjectFileError::TooNew(header.schema_version));
    }
    migrate(&mut value, header.schema_version);
    let project: Project = serde_json::from_value(value["project"].take())
        .map_err(|e| ProjectFileError::Damaged(e.to_string()))?;
    validate(&project)?;
    Ok(project)
}

/// Upgrade an older file's JSON in place, one schema step at a time.
fn migrate(value: &mut serde_json::Value, from: u32) {
    for step in from..SCHEMA_VERSION {
        #[allow(clippy::single_match)]
        match step {
            // 1 → 2: an asset's one optional audio stream becomes the list of
            // all of them. (Clips' new `audio_stream` defaults to 0, the
            // stream schema 1 played.)
            1 => {
                if let Some(assets) = value["project"]["assets"].as_object_mut() {
                    for asset in assets.values_mut() {
                        let info = &mut asset["info"];
                        if info.is_object() {
                            let audio = info["audio"].take();
                            info["audio"] = if audio.is_null() { serde_json::json!([]) } else { serde_json::json!([audio]) };
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema-1 file, as the previous version wrote it.
    const V1: &str = r#"{
      "format": "ve-project", "schema_version": 1,
      "project": {
        "schema_version": 1, "name": "old",
        "assets": {
          "01M4DZH8X9EY43WF5BDZA6X0T3": {
            "id": "01M4DZH8X9EY43WF5BDZA6X0T3", "name": "a.mp4", "media": "file:/a.mp4",
            "info": { "duration": 254016000000, "video": null,
                      "audio": { "sample_rate": 48000, "channels": 2, "codec": "aac" } }
          },
          "01M4DZH8XCES9V3BSXCXWDZTD6": {
            "id": "01M4DZH8XCES9V3BSXCXWDZTD6", "name": "b.mov", "media": "file:/b.mov",
            "info": { "duration": 254016000000, "video": null, "audio": null }
          },
          "01M4DZH8XCES9V3BSXCXWDZTD7": {
            "id": "01M4DZH8XCES9V3BSXCXWDZTD7", "name": "c.mov", "media": "file:/c.mov", "info": null
          }
        },
        "sequences": {}, "active_sequence": null
      }
    }"#;

    #[test]
    fn schema_1_files_open() {
        let p = read(&mut V1.as_bytes()).unwrap();
        let info = |name: &str| p.assets.values().find(|a| a.name == name).unwrap().info.clone();
        let a = info("a.mp4").unwrap().audio;
        assert_eq!((a.len(), a[0].channels, a[0].layout.as_str()), (1, 2, ""));
        assert!(info("b.mov").unwrap().audio.is_empty());
        assert!(info("c.mov").is_none());
    }
}
