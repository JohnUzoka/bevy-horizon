//! Validated Naga IR → GLSL 450 with deko3d bindings → DKSH, compiled on the console by UAM.
use std::{
    collections::HashMap,
    ffi::CString,
    sync::{Arc, Mutex},
};

use naga::back::glsl;
use wgpu_hal as hal;
use wgpu_types as wgt;

use super::ffi;

/// A GLSL combined sampler: the deko3d texture binding it occupies and the WGSL resources it pairs.
#[derive(Debug, Clone, Copy)]
pub struct CombinedTexture {
    pub slot: u8,
    pub texture: naga::ResourceBinding,
    pub sampler: Option<naga::ResourceBinding>,
}

#[derive(Debug)]
pub struct Stage {
    pub dksh: Arc<[u8]>,
    pub textures: Vec<CombinedTexture>,
}

/// Compiled DKSH by stage and GLSL text. Pipelines that share a shader (most of Bevy's) compile it
/// once, and a later launch reads it from the SD card (shader_cache).
#[derive(Debug, Default)]
pub struct Cache(Mutex<HashMap<(i32, String), Arc<[u8]>>>);

impl Cache {
    pub fn compile(&self, stage: i32, glsl: &str) -> Result<Arc<[u8]>, String> {
        let key = (stage, glsl.to_owned());
        if let Some(dksh) = self.0.lock().unwrap().get(&key) {
            return Ok(dksh.clone());
        }
        let dksh = super::shader_cache::get_or_compile(stage, glsl, || {
            let source = CString::new(glsl).map_err(|_| "GLSL contains a NUL byte".to_string())?;
            let mut size = 0;
            let raw = unsafe { ffi::dkp_uam_compile(stage, source.as_ptr(), &mut size) };
            if raw.is_null() {
                return Err("UAM rejected the GLSL (its messages go to stderr)".into());
            }
            let dksh: Arc<[u8]> = Arc::from(unsafe { std::slice::from_raw_parts(raw, size) });
            unsafe { ffi::dkp_uam_free(raw.cast()) };
            Ok(dksh)
        })?;
        self.0.lock().unwrap().insert(key, dksh.clone());
        Ok(dksh)
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// The fragment program for pipelines without one (depth-only passes).
pub const EMPTY_FRAGMENT: &str = "#version 450\nvoid main() {}\n";

/// The binding immediates are rewritten to: a uniform buffer the encoder fills per draw.
pub const IMMEDIATES: naga::ResourceBinding = naga::ResourceBinding { group: u32::MAX, binding: 0 };

/// For a void entry point whose body ends in an `if` (or a block) whose sides end in a bare
/// `return`: those returns removed, and one `return` after it, which they now fall through to.
/// Naga's WGSL front end ends every side of a trailing `if` with a `return`
/// (`proc::ensure_block_returns`), so `if c { discard; }` at the end of a fragment entry becomes
/// `if (c) { discard; } else { return; }`; UAM compiles that `main` without an EXIT, the
/// discarding invocations branch back into the program and never finish, and the queue times out
/// (Daybreak's entity shadow pass: `horizon_entity_shadow_fs`). `if (c) { discard; } return;` gets
/// its EXIT. Returns whether it changed the body.
fn end_with_return(body: &mut naga::Block) -> bool {
    use naga::Statement as S;
    fn strip(block: &mut naga::Block) -> bool {
        if matches!(block.last(), Some(S::Return { value: None })) {
            let len = block.len();
            block.cull(len - 1..);
            return true;
        }
        match block.last_mut() {
            Some(S::If { accept, reject, .. }) => strip(accept) | strip(reject),
            Some(S::Block(inner)) => strip(inner),
            _ => false,
        }
    }
    if !matches!(body.last(), Some(S::If { .. } | S::Block(_))) || !strip(body) {
        return false;
    }
    body.push(S::Return { value: None }, naga::Span::default());
    true
}

pub fn compile(
    cache: &Cache,
    stage: &hal::ProgrammableStage<'_, super::ShaderModule>,
    kind: naga::ShaderStage,
    binding_map: &glsl::BindingMap,
) -> Result<Stage, hal::PipelineError> {
    let mask = match kind {
        naga::ShaderStage::Vertex => wgt::ShaderStages::VERTEX,
        naga::ShaderStage::Fragment => wgt::ShaderStages::FRAGMENT,
        _ => wgt::ShaderStages::COMPUTE,
    };
    let error = |message: String| hal::PipelineError::Linkage(mask, message);
    let shader = &stage.module.0;
    let (mut module, mut info) = naga::back::pipeline_constants::process_overrides(
        &shader.module, &shader.info, Some((kind, stage.entry_point)), stage.constants,
    ).map_err(|e| hal::PipelineError::PipelineConstants(mask, e.to_string()))?;

    // UAM compiles a `main` that ends in an `if` with `discard` on one side and `return` on the
    // other into a program with no EXIT, which the discarding invocations never leave (see
    // `end_with_return`). End such a body with one `return` of its own.
    if let Some(index) = module.entry_points.iter().position(|e| e.stage == kind && e.name == stage.entry_point)
        && module.entry_points[index].function.result.is_none()
    {
        let mut body = module.entry_points[index].function.body.clone();
        if end_with_return(&mut body) {
            module.to_mut().entry_points[index].function.body = body;
        }
    }

    // deko3d has no push constants: immediates become a uniform buffer the encoder fills.
    if module.global_variables.iter().any(|(_, var)| var.space == naga::AddressSpace::Immediate) {
        let owned = module.to_mut();
        for (_, var) in owned.global_variables.iter_mut() {
            if var.space == naga::AddressSpace::Immediate {
                var.space = naga::AddressSpace::Uniform;
                var.binding = Some(IMMEDIATES);
            }
        }
        let validated = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(owned)
            .map_err(|e| error(format!("immediates as a uniform buffer: {}", e.emit_to_string(""))))?;
        info = std::borrow::Cow::Owned(validated);
    }

    let options = glsl::Options {
        version: glsl::Version::Desktop(450),
        // deko3d's device uses wgpu's conventions (upper-left origin, 0..1 depth), so no
        // coordinate adjustment. Draw parameters make instance_index include first_instance.
        writer_flags: glsl::WriterFlags::DRAW_PARAMETERS,
        binding_map: binding_map.clone(),
        zero_initialize_workgroup_memory: stage.zero_initialize_workgroup_memory,
    };
    let pipeline = glsl::PipelineOptions { shader_stage: kind, entry_point: stage.entry_point.to_owned(), multiview: None };
    let bounds = naga::proc::BoundsCheckPolicies {
        index: naga::proc::BoundsCheckPolicy::Restrict,
        buffer: naga::proc::BoundsCheckPolicy::ReadZeroSkipWrite,
        image_load: naga::proc::BoundsCheckPolicy::ReadZeroSkipWrite,
        binding_array: naga::proc::BoundsCheckPolicy::Restrict,
    };
    let mut source = String::new();
    glsl::Writer::new(&mut source, &module, &info, &options, &pipeline, bounds)
        .and_then(|mut writer| writer.write())
        .map_err(|e| error(format!("GLSL for {}: {e}", stage.entry_point)))
        .and_then(|reflection| finish(cache, kind, stage.entry_point, &module, binding_map, reflection, source, error))
}

#[allow(clippy::too_many_arguments)]
fn finish(
    cache: &Cache,
    kind: naga::ShaderStage,
    entry_point: &str,
    module: &naga::Module,
    binding_map: &glsl::BindingMap,
    reflection: glsl::ReflectionInfo,
    source: String,
    error: impl Fn(String) -> hal::PipelineError,
) -> Result<Stage, hal::PipelineError> {
    let mut textures: Vec<CombinedTexture> = Vec::new();
    for mapping in reflection.texture_mapping.values() {
        let var = &module.global_variables[mapping.texture];
        // Storage images bind by their own slots; only sampled textures pair with samplers.
        if matches!(module.types[var.ty].inner, naga::TypeInner::Image { class: naga::ImageClass::Storage { .. }, .. }) {
            continue;
        }
        let binding = |handle: naga::Handle<naga::GlobalVariable>| module.global_variables[handle].binding.clone().expect("bound resource");
        let texture = binding(mapping.texture);
        let sampler = mapping.sampler.map(binding);
        let slot = binding_map[&texture];
        // GLSL combines each texture with its sampler at the texture's binding. A texture
        // sampled through two samplers would need two bindings.
        if let Some(other) = textures.iter().find(|t| t.slot == slot) {
            if other.sampler != sampler {
                return Err(error(format!("{entry_point}: one texture used with two samplers")));
            }
            continue;
        }
        textures.push(CombinedTexture { slot, texture, sampler });
    }

    let (uam_stage, name) = match kind {
        naga::ShaderStage::Vertex => (ffi::UAM_VERTEX, "vertex"),
        naga::ShaderStage::Fragment => (ffi::UAM_FRAGMENT, "fragment"),
        _ => (ffi::UAM_COMPUTE, "compute"),
    };
    let dksh = cache.compile(uam_stage, &source).map_err(|e| {
        log::error!("{entry_point} {name}: {e}\n{source}");
        error(format!("{entry_point}: {e}"))
    })?;
    Ok(Stage { dksh, textures })
}
