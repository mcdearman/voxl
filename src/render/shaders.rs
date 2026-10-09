//! Shader sources, and reloading them while the app runs.
//!
//! Every WGSL file the engine uses is compiled into the binary. In a development build the
//! file on disk is used instead when it is there, and watched: save a shader and the
//! pipelines built from it are rebuilt within a moment, without restarting. If the new source
//! doesn't compile, or no longer fits its pipeline, the error is logged and the old pipelines
//! stay.
//!
//! Reading from disk is on in debug builds and off in release builds; `MIRA_HOT_SHADERS=1`
//! or `=0` overrides either way.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use super::Gpu;
use crate::ecs::World;

/// A WGSL file: its name, the copy compiled into the binary, and where its source lives.
/// Make one with [`shader!`](crate::shader).
#[derive(Clone, Copy, Debug)]
pub struct Shader {
    pub name: &'static str,
    pub embedded: &'static str,
    /// The Rust file that named it, from the crate root. The shader sits beside it.
    pub beside: &'static str,
    pub manifest_dir: &'static str,
}

/// Names a WGSL file beside the current source file, like `include_str!`, but so that it can
/// be reloaded while the app runs.
///
/// ```ignore
/// let module = mira::shader!("water.wgsl").module(&gpu.device);
/// ```
#[macro_export]
macro_rules! shader {
    ($name:literal) => {
        $crate::render::Shader {
            name: $name,
            embedded: include_str!($name),
            beside: file!(),
            manifest_dir: env!("CARGO_MANIFEST_DIR"),
        }
    };
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("MIRA_HOT_SHADERS").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => cfg!(debug_assertions),
    })
}

/// The files that have been read from disk, with the modification time each was read at.
fn watched() -> &'static Mutex<HashMap<PathBuf, Option<SystemTime>>> {
    static WATCHED: OnceLock<Mutex<HashMap<PathBuf, Option<SystemTime>>>> = OnceLock::new();
    WATCHED.get_or_init(Default::default)
}

fn modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

impl Shader {
    fn path(&self) -> PathBuf {
        PathBuf::from(self.manifest_dir)
            .join(self.beside)
            .with_file_name(self.name)
    }

    /// The shader's text: the file on disk when shaders are being reloaded and it exists,
    /// otherwise the copy compiled in.
    pub fn source(&self) -> String {
        if enabled() {
            let path = self.path();
            if let Ok(text) = std::fs::read_to_string(&path) {
                let stamp = modified(&path);
                watched().lock().unwrap().insert(path, stamp);
                return text;
            }
        }
        self.embedded.to_owned()
    }

    /// Compiles the shader by itself.
    pub fn module(&self, device: &wgpu::Device) -> wgpu::ShaderModule {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(self.name),
            source: wgpu::ShaderSource::Wgsl(self.source().into()),
        })
    }
}

/// Whether a shader read from disk has changed and then stopped changing. Checks a few
/// times a second at most; a change is reported one check after it was last seen, so a file
/// an editor is still writing isn't read half-finished.
fn any_changed() -> bool {
    struct Poll {
        last: Option<Instant>,
        settling: bool,
    }
    static POLL: Mutex<Poll> = Mutex::new(Poll {
        last: None,
        settling: false,
    });
    let mut poll = POLL.lock().unwrap();
    if poll
        .last
        .is_some_and(|t| t.elapsed() < Duration::from_millis(150))
    {
        return false;
    }
    poll.last = Some(Instant::now());
    let mut watched = watched().lock().unwrap();
    let mut changed = false;
    for (path, stamp) in watched.iter_mut() {
        let now = modified(path);
        if now != *stamp {
            *stamp = now;
            changed = true;
        }
    }
    let settled = poll.settling && !changed;
    poll.settling = changed;
    settled
}

/// Puts rebuilt pipelines in place. Returned by a [`Rebuild`] once it has built them.
pub type Install = Box<dyn FnOnce(&mut World)>;

/// Rebuilds, from the current shader sources, whatever a renderer made from shaders, without
/// touching the world yet. It returns how to put the result in place, which only happens if
/// every rebuild succeeded: a shader that fails to compile leaves all the old pipelines.
pub type Rebuild = fn(&World) -> Option<Install>;

/// What to rebuild when a shader changes. A plugin with its own pipelines pushes a
/// [`Rebuild`] here so that its shaders reload with the engine's.
#[derive(Default)]
pub struct ShaderReload(pub Vec<Rebuild>);

/// Rebuilds every pipeline if a shader file has changed.
pub(crate) fn reload_changed(world: &mut World) {
    if !enabled() || !world.contains_resource::<Gpu>() || !any_changed() {
        return;
    }
    let device = world.resource::<Gpu>().device.clone();
    // Collect validation errors instead of letting wgpu panic on the first one.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let rebuilds = world.resource::<ShaderReload>().0.clone();
    let installs: Vec<Install> = rebuilds
        .iter()
        .filter_map(|rebuild| rebuild(world))
        .collect();
    match pollster::block_on(scope.pop()) {
        Some(error) => log::error!("shader not reloaded; still using the old one:\n{error}"),
        None => {
            for install in installs {
                install(world);
            }
            log::info!("reloaded shaders");
        }
    }
}
