//! Shader composition (naga_oil) and the device's programs: modules
//! composed with the format-dependent shader defs, and each stacker's
//! pipelines built once per device.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use naga_oil::compose::{Composer, NagaModuleDescriptor, ShaderDefValue};

use super::GpuContext;

impl GpuContext {
    /// Validation capabilities for composition: without the 16-bit-norm
    /// capability naga_oil rejects the upload's rgba16unorm target even on
    /// a device that supports it.
    fn naga_capabilities(&self) -> wgpu::naga::valid::Capabilities {
        let mut caps = wgpu::naga::valid::Capabilities::default();
        if self.format_support.rgba16unorm_storage {
            caps |= wgpu::naga::valid::Capabilities::STORAGE_TEXTURE_16BIT_NORM_FORMATS;
        }
        caps
    }

    /// Shader defs evaluated by `#ifdef` blocks, derived from format support.
    fn shader_defs(&self) -> HashMap<String, ShaderDefValue> {
        let mut defs = HashMap::new();
        if self.format_support.rgba16unorm_storage {
            defs.insert("SOURCE_RGBA16UNORM".into(), ShaderDefValue::Bool(true));
        }
        if self.format_support.r8_storage {
            defs.insert("MAP_R8".into(), ShaderDefValue::Bool(true));
        }
        defs
    }

    /// The one `T` on this device, built by `make` on first ask: a
    /// stacker's pipelines, tens of milliseconds to compile. Every user
    /// shares it, so `T` holds only what never changes. Two threads asking
    /// at once may both build it; the first stored is kept.
    pub fn programs<T: Any + Send + Sync>(&self, make: impl FnOnce(&Self) -> T) -> Arc<T> {
        let key = TypeId::of::<T>();
        let made = |p: &Arc<dyn Any + Send + Sync>| {
            Arc::clone(p).downcast::<T>().expect("stored under its own TypeId")
        };
        if let Some(p) = self.programs.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return made(p);
        }
        // Built outside the lock: compiling takes long, and `make` may ask
        // for programs of its own.
        let built: Arc<dyn Any + Send + Sync> = Arc::new(make(self));
        let mut programs = self.programs.lock().unwrap_or_else(|e| e.into_inner());
        made(programs.entry(key).or_insert(built))
    }

    /// Create a shader module, evaluating `#ifdef` blocks against GPU format
    /// support (SOURCE_RGBA16UNORM, MAP_R8).
    pub fn create_shader_module(&self, source: &str, label: &str) -> wgpu::ShaderModule {
        self.create_shader_module_composed(source, label, &[])
    }

    /// [`Self::create_shader_module`] with `defs` defined as well, for a
    /// shader compiled in more than one variant on the same device.
    pub fn create_shader_module_with(
        &self,
        source: &str,
        label: &str,
        defs: &[&str],
    ) -> wgpu::ShaderModule {
        let mut all = self.shader_defs();
        for def in defs {
            all.insert((*def).into(), ShaderDefValue::Bool(true));
        }
        self.create_module(
            compose_module(source, label, &[], &all, self.naga_capabilities()),
            label,
        )
    }

    /// [`Self::create_shader_module`] with `shared_modules` (source, virtual
    /// path) registered for `#import`, named by their `#define_import_path`.
    pub fn create_shader_module_composed(
        &self,
        source: &str,
        label: &str,
        shared_modules: &[(&str, &str)],
    ) -> wgpu::ShaderModule {
        let module = compose_module(
            source,
            label,
            shared_modules,
            &self.shader_defs(),
            self.naga_capabilities(),
        );
        self.create_module(module, label)
    }

    /// The device's module for a composed `module`.
    fn create_module(&self, module: wgpu::naga::Module, label: &str) -> wgpu::ShaderModule {
        // naga's loop guard keeps Metal from unrolling loops; without it
        // the output is the same, and faster. Bounds checks stay on.
        let checks = wgpu::ShaderRuntimeChecks {
            force_loop_bounding: false,
            ..wgpu::ShaderRuntimeChecks::checked()
        };
        // SAFETY: `force_loop_bounding: false` requires that no shader can
        // loop forever. Ours are all counted `for` loops whose bounds come
        // from constants, uniforms or texture sizes; keep it that way (no
        // data-dependent `loop`/`while`) for anything composed here.
        unsafe {
            self.device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
                },
                checks,
            )
        }
    }
}

/// Compose `source` with `shared_modules` into one naga module against
/// `defs`. Panics on an error: the sources are built in, so it is a bug.
fn compose_module(
    source: &str,
    label: &str,
    shared_modules: &[(&str, &str)],
    defs: &HashMap<String, ShaderDefValue>,
    caps: wgpu::naga::valid::Capabilities,
) -> wgpu::naga::Module {
    let mut composer = Composer::default().with_capabilities(caps);
    for (module_source, path) in shared_modules {
        composer
            .add_composable_module(naga_oil::compose::ComposableModuleDescriptor {
                source: module_source,
                file_path: path,
                shader_defs: defs.clone(),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("Failed to add shared shader '{path}': {e:?}"));
    }
    composer
        .make_naga_module(NagaModuleDescriptor {
            source,
            file_path: label,
            shader_defs: defs.clone(),
            ..Default::default()
        })
        .unwrap_or_else(|e| panic!("Shader composition failed for '{label}': {e:?}"))
}
