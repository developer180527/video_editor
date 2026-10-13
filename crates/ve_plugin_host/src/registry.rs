use thiserror::Error;
use ve_model::{PluginApi, PluginRef};

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("{0}: built for ABI {1}, this host speaks {2}")]
    AbiMismatch(String, u32, u32),
    #[error("{0}: malformed descriptor: {1}")]
    Malformed(String, String),
    #[error("the plugin declined to load")]
    Declined,
    #[error(transparent)]
    Library(#[from] ve_ports::LibraryError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    Filter,
    /// An effect on a clip's sound (run by the mixer).
    AudioFilter,
    Transition,
    Generator,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Vec2,
    Color,
    Choice(Vec<String>),
    /// Text, with its default (a title's words).
    Text(String),
}

/// A parameter as the host's UI and keyframing see it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamInfo {
    pub id: String,
    pub label: String,
    pub kind: ParamKind,
    pub min: f64,
    pub max: f64,
    pub default: [f64; 4],
    pub animatable: bool,
}

/// Everything known about one effect, whatever API it came through.
#[derive(Clone)]
pub struct EffectInfo {
    pub plugin: PluginRef,
    pub name: String,
    pub category: String,
    pub kind: EffectKind,
    pub params: Vec<ParamInfo>,
    /// GPU implementation as WGSL, when the plugin has one.
    pub wgsl: Option<String>,
    pub implementation: Implementation,
    /// Its shader may sample below level 0 (wide blurs, zooms): the host
    /// builds mip chains only for effects that do (`wgsl::reads_lower_levels`).
    pub reads_levels: bool,
}

#[derive(Clone)]
pub enum Implementation {
    ShaderOnly,
    /// Implemented by the engine itself (Motion, Opacity, Volume, Panner).
    Intrinsic,
    /// Run by the mixer on a clip's sound (the built-in audio effects).
    Mixer,
}

#[derive(Default)]
pub struct Registry {
    effects: Vec<EffectInfo>,
}

impl Registry {
    pub fn add(&mut self, e: EffectInfo) {
        // A later registration of the same id and major version replaces the
        // earlier: a user-installed plugin overrides a bundled one.
        self.effects.retain(|x| x.plugin != e.plugin);
        self.effects.push(e);
    }

    pub fn effects(&self) -> &[EffectInfo] {
        &self.effects
    }

    pub fn find(&self, r: &PluginRef) -> Option<&EffectInfo> {
        self.effects.iter().find(|e| &e.plugin == r)
    }

    pub fn find_id(&self, api: PluginApi, id: &str) -> Option<&EffectInfo> {
        self.effects.iter().find(|e| e.plugin.api == api && e.plugin.id == id)
    }
}
