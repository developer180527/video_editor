//! Native `ve_plugin.h` plugins.
//!
//! In-process native code is trusted code: a crash in a plugin is a crash in
//! the app. That is the price of speed; untrusted code belongs in a WASM or
//! process add-on.

use std::ffi::{c_char, c_void, CStr};
use std::sync::Arc;
use ve_model::{PluginApi, PluginRef};
use ve_plugin_abi as abi;
use ve_ports::NativeLibrary;

use crate::{EffectInfo, EffectKind, Implementation, ParamInfo, ParamKind, PluginError};

unsafe extern "C" fn host_log(level: abi::VeLogLevel, msg: *const c_char) {
    if !msg.is_null() {
        eprintln!("[plugin {level:?}] {}", CStr::from_ptr(msg).to_string_lossy());
    }
}

unsafe extern "C" fn host_extension(_name: *const c_char) -> *const c_void {
    std::ptr::null() // no extensions in v1
}

static HOST: abi::VeHost = abi::VeHost {
    struct_size: std::mem::size_of::<abi::VeHost>() as u32,
    abi_version: abi::VE_ABI_VERSION,
    log: Some(host_log),
    get_extension: Some(host_extension),
};

/// One effect inside a loaded native library. Holds the library open.
pub struct NativeEffect {
    _lib: Arc<dyn NativeLibrary>,
    /// Our copy, at our layout (see `read_versioned`): reading the plugin's
    /// own struct could run past the end of an older, smaller one.
    desc: abi::VeEffectDesc,
}

// The descriptor's pointers are to immutable static data in the plugin;
// instances are created per use.
unsafe impl Send for NativeEffect {}
unsafe impl Sync for NativeEffect {}

/// Read the plugin in `lib` and describe its effects.
///
/// `static_name` is the plugin's link name when it is statically linked.
pub fn load(lib: Arc<dyn NativeLibrary>, static_name: Option<&str>) -> Result<Vec<EffectInfo>, PluginError> {
    let lib_name = lib.name().to_string();
    let sym = lib.symbol(&abi::entry_symbol(static_name))?;
    let entry: abi::VePluginEntryFn = unsafe { std::mem::transmute(sym) };
    let desc = unsafe { entry(&HOST) };
    if desc.is_null() {
        return Err(PluginError::Declined);
    }
    let bad = |what: &str| PluginError::Malformed(lib_name.clone(), what.into());
    let desc = unsafe { read_versioned(desc, abi::VE_PLUGIN_DESC_MIN_SIZE) }.ok_or_else(|| bad("VePluginDesc too small"))?;
    if desc.abi_version > abi::VE_ABI_VERSION || desc.abi_version == 0 {
        return Err(PluginError::AbiMismatch(lib_name, desc.abi_version, abi::VE_ABI_VERSION));
    }
    let mut out = Vec::new();
    for i in 0..desc.effect_count as usize {
        let e = unsafe { *desc.effects.add(i) };
        if e.is_null() {
            return Err(bad("null effect"));
        }
        let ed = unsafe { read_versioned(e, abi::VE_EFFECT_DESC_MIN_SIZE) }.ok_or_else(|| bad("VeEffectDesc too small"))?;
        let id = cstr(ed.id).ok_or_else(|| bad("effect without id"))?;
        // An inline array of structs: its stride is the plugin's struct
        // size (the first element's), not ours.
        let stride = match ed.param_count {
            0 => 0,
            _ if ed.params.is_null() => return Err(bad("null parameters")),
            _ => (unsafe { *(ed.params as *const u32) }) as usize,
        };
        let params = (0..ed.param_count as usize)
            .map(|p| {
                let at = unsafe { (ed.params as *const u8).add(p * stride) } as *const abi::VeParamDesc;
                let pd = unsafe { read_versioned(at, abi::VE_PARAM_DESC_MIN_SIZE) }.ok_or_else(|| bad("VeParamDesc too small"))?;
                param_info(&pd).ok_or_else(|| bad("parameter without an id, or of a type this host does not know"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let wgsl = cstr(ed.wgsl);
        let (name, category, major_version) = (cstr(ed.name).unwrap_or(id.clone()), cstr(ed.category).unwrap_or_default(), ed.major_version);
        let kind = match ed.kind {
            abi::VeEffectKind::Filter => EffectKind::Filter,
            abi::VeEffectKind::Transition => EffectKind::Transition,
            abi::VeEffectKind::Generator => EffectKind::Generator,
            other => return Err(bad(&format!("unknown effect kind {}", other.0))),
        };
        let implementation = if ed.render_cpu.is_some() {
            Implementation::Native(Arc::new(NativeEffect { _lib: lib.clone(), desc: ed }))
        } else if wgsl.is_some() {
            Implementation::ShaderOnly
        } else {
            return Err(bad("effect with neither wgsl nor render_cpu"));
        };
        out.push(EffectInfo {
            plugin: PluginRef { api: PluginApi::Native, id, major_version },
            name,
            category,
            kind,
            params,
            wgsl,
            implementation,
        });
    }
    Ok(out)
}

/// Read a struct that begins with `struct_size`, whatever version the plugin
/// was built against: fields both sides know are copied; fields an older,
/// smaller struct lacks are zero (null / `None` / the 0 variant — "not
/// provided", by the ABI's rule for appended fields); fields a newer, larger
/// struct adds are ignored. `None` if smaller than `min` (the v1 layout).
///
/// # Safety
/// `p` points to a readable struct of at least its own `struct_size` bytes.
unsafe fn read_versioned<T>(p: *const T, min: usize) -> Option<T> {
    let size = unsafe { *(p as *const u32) } as usize;
    if size < min {
        return None;
    }
    let mut out = std::mem::MaybeUninit::<T>::zeroed();
    unsafe {
        std::ptr::copy_nonoverlapping(p as *const u8, out.as_mut_ptr() as *mut u8, size.min(std::mem::size_of::<T>()));
        Some(out.assume_init())
    }
}

fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn param_info(p: &abi::VeParamDesc) -> Option<ParamInfo> {
    let kind = match p.ty {
        abi::VeParamType::Bool => ParamKind::Bool,
        abi::VeParamType::Int => ParamKind::Int,
        abi::VeParamType::Float => ParamKind::Float,
        abi::VeParamType::Vec2 => ParamKind::Vec2,
        abi::VeParamType::Color => ParamKind::Color,
        abi::VeParamType::Choice => ParamKind::Choice(
            (0..p.choice_count as usize).filter_map(|i| cstr(unsafe { *p.choices.add(i) })).collect(),
        ),
        // A type from a newer ABI: the plugin is refused, not misread.
        _ => return None,
    };
    Some(ParamInfo {
        id: cstr(p.id)?,
        label: cstr(p.label).unwrap_or_default(),
        kind,
        min: p.min,
        max: p.max,
        default: p.default_value,
        animatable: p.flags & abi::VE_PARAM_ANIMATABLE != 0,
    })
}

/// An RGBA f32 image owned by the host.
pub struct CpuImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

impl CpuImage {
    pub fn new(width: u32, height: u32) -> Self {
        CpuImage { width, height, pixels: vec![0.0; (width * height * 4) as usize] }
    }

    fn view(&mut self) -> abi::VeImage {
        abi::VeImage {
            struct_size: std::mem::size_of::<abi::VeImage>() as u32,
            width: self.width,
            height: self.height,
            stride: self.width as usize * 16,
            pixels: self.pixels.as_mut_ptr(),
        }
    }
}

impl NativeEffect {
    /// Render on the CPU. `params` are the values at this time, in the
    /// plugin's declaration order.
    pub fn render_cpu(
        &self,
        time: f64,
        params: &[[f64; 4]],
        inputs: &mut [&mut CpuImage],
        output: &mut CpuImage,
    ) -> Result<(), PluginError> {
        let d = &self.desc;
        let render = d.render_cpu.ok_or(PluginError::Unavailable("CPU rendering"))?;
        let instance = d.create.map(|c| unsafe { c(&HOST) }).unwrap_or(std::ptr::null_mut());
        let args = abi::VeRenderArgs {
            struct_size: std::mem::size_of::<abi::VeRenderArgs>() as u32,
            time,
            frame_duration: 0.0,
            scale: 1.0,
            params: abi::VeParamValues { count: params.len() as u32, values: params.as_ptr() },
            progress: 0.0,
        };
        let views: Vec<abi::VeImage> = inputs.iter_mut().map(|i| i.view()).collect();
        let ptrs: Vec<*const abi::VeImage> = views.iter().map(|v| v as *const _).collect();
        let mut out = output.view();
        let code = unsafe { render(instance, &args, ptrs.as_ptr(), ptrs.len() as u32, &mut out) };
        if let Some(destroy) = d.destroy {
            unsafe { destroy(instance) };
        }
        if code == 0 {
            Ok(())
        } else {
            Err(PluginError::RenderFailed(code))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct V1 {
        struct_size: u32,
        a: u64,
    }

    #[repr(C)]
    struct V2 {
        struct_size: u32,
        a: u64,
        b: *const c_char,
        f: Option<extern "C" fn()>,
    }

    #[test]
    fn older_structs_read_with_new_fields_absent() {
        let old = V1 { struct_size: std::mem::size_of::<V1>() as u32, a: 7 };
        let new: V2 = unsafe { read_versioned(&old as *const V1 as *const V2, std::mem::size_of::<V1>()) }.unwrap();
        assert_eq!(new.a, 7);
        assert!(new.b.is_null() && new.f.is_none());
    }

    #[test]
    fn newer_structs_read_what_we_know() {
        let newer = V2 { struct_size: std::mem::size_of::<V2>() as u32, a: 9, b: std::ptr::null(), f: None };
        let ours: V1 = unsafe { read_versioned(&newer as *const V2 as *const V1, std::mem::size_of::<V1>()) }.unwrap();
        assert_eq!(ours.a, 9);
    }

    #[test]
    fn structs_below_version_one_are_refused() {
        let tiny = V1 { struct_size: 4, a: 0 };
        assert!(unsafe { read_versioned(&tiny as *const V1, std::mem::size_of::<V1>()) }.is_none());
    }
}
