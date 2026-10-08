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
fn migrate(_value: &mut serde_json::Value, from: u32) {
    #[allow(clippy::match_single_binding)]
    for _step in from..SCHEMA_VERSION {
        match _step {
            // 1 => { /* 1 → 2 */ }
            _ => {}
        }
    }
}
