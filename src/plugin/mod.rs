//! Native plugins: shared libraries, in any language that can export C functions, that add
//! components and systems to a running app and are reloaded when they are rebuilt.
//!
//! The contract is `include/voxl.h`. A plugin exports `voxl_plugin_abi_version`,
//! `voxl_plugin_load` and (optionally) `voxl_plugin_unload`; the engine hands it a table of
//! functions for defining components, adding systems and, inside a system, walking a query
//! and queueing changes. Rust plugins use the `voxl_plugin` crate over the same interface.
//!
//! # Hot reload
//!
//! Each plugin's file is checked a few times a second. Once a changed file has stopped
//! changing, the new library is loaded, the old one is told to unload, and the plugin's load
//! function runs again. Its new systems take the places of the old ones of the same name, so
//! the order systems run in is unchanged. Component values and state blocks live in the
//! engine and are untouched, so the world carries on from where it was with new code. If the
//! new library can't be opened, the old one keeps running.
//!
//! Each version is loaded from a private copy of the file, since a library that is in use
//! can't be reliably overwritten or re-opened under the same path.
//!
//! # Trust
//!
//! A plugin is native code in the engine's process: it can crash it or corrupt memory, and
//! nothing here can prevent that. Load only plugins you would be willing to link statically.

mod api;
mod system;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{bail, Context as _};
use libloading::Library;
use voxl_plugin::sys::{self, VoxlApi, VoxlApp};

use self::api::{Registrar, StateBlock};
use crate::{
    app::{App, Stage},
    ecs::{BoxedSystem, SystemOwner},
};

/// The stages a plugin can add systems to.
const STAGES: [Stage; 7] = [
    Stage::Startup,
    Stage::First,
    Stage::PreUpdate,
    Stage::FixedUpdate,
    Stage::Update,
    Stage::PostUpdate,
    Stage::Last,
];

type LoadFn = unsafe extern "C" fn(*const VoxlApi, *mut VoxlApp) -> i32;

/// A file's modification time and size: enough to tell that it was rebuilt.
type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The file name a shared library called `name` has on this platform: `libname.dylib`,
/// `libname.so` or `name.dll`.
pub fn library_file_name(name: &str) -> String {
    format!(
        "{}{name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// Where Cargo puts the workspace's `cdylib` crates for the running build profile: the
/// directory of the current executable, or its parent for examples and tests.
pub fn cargo_library_path(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().unwrap_or(Path::new(".")).to_owned();
    if dir.ends_with("examples") || dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(library_file_name(name))
}

/// Deletes a private library copy when dropped, unless it is kept.
struct TempCopy(Option<PathBuf>);

impl TempCopy {
    fn keep(mut self) -> PathBuf {
        self.0.take().unwrap()
    }
}

impl Drop for TempCopy {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Removes copies left behind by apps that were killed before they could clean up.
fn sweep_stale_copies(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let day = Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

struct Loaded {
    name: String,
    source: PathBuf,
    owner: SystemOwner,
    library: Option<Library>,
    /// The private copy `library` was opened from.
    copy: Option<PathBuf>,
    /// The version of the file that is loaded.
    current: Option<Stamp>,
    /// A newer version seen on the last check, waiting to see whether it is still changing.
    settling: Option<Stamp>,
    /// A version that failed to load, so it isn't retried until the file changes again.
    rejected: Option<Stamp>,
    /// Runtime-defined components whose destructors live in `library`.
    components: Vec<u32>,
    generation: u32,
    /// The plugin asked never to be unmapped (`VOXL_PLUGIN_KEEP_LOADED`).
    keep_loaded: bool,
}

impl Loaded {
    /// Lets go of a library version: closes it, or for a plugin whose runtime can't be
    /// unloaded, leaves it mapped and merely stops using it.
    fn close(&mut self) {
        if let Some(library) = self.library.take() {
            if self.keep_loaded {
                std::mem::forget(library);
            }
        }
        if let Some(copy) = self.copy.take() {
            let _ = std::fs::remove_file(copy);
        }
    }
}

/// The app's native plugins. Reach it through `App::native_plugins`.
pub struct NativePlugins {
    plugins: Vec<Loaded>,
    states: HashMap<String, StateBlock>,
    last_check: Option<Instant>,
    /// How often plugin files are checked for changes.
    pub check_interval: Duration,
    /// Set to false to stop reloading changed plugins.
    pub hot_reload: bool,
}

impl Drop for Loaded {
    fn drop(&mut self) {
        self.close();
    }
}

impl Default for NativePlugins {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            states: HashMap::new(),
            last_check: None,
            check_interval: Duration::from_millis(200),
            hot_reload: true,
        }
    }
}

impl NativePlugins {
    /// The names of the loaded plugins, with how many times each has been loaded.
    pub fn loaded(&self) -> impl Iterator<Item = (&str, u32)> {
        self.plugins
            .iter()
            .filter(|p| p.library.is_some())
            .map(|p| (p.name.as_str(), p.generation))
    }
}

impl App {
    /// Loads a native plugin from a shared library and keeps it up to date as the file
    /// changes. Call it after adding the engine plugins whose components it uses.
    pub fn load_native_plugin(&mut self, path: impl AsRef<Path>) -> anyhow::Result<&mut Self> {
        let source = path.as_ref().to_owned();
        let name = source
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.strip_prefix(std::env::consts::DLL_PREFIX).unwrap_or(s))
            .context("plugin path has no file name")?
            .to_owned();
        if self.native.plugins.iter().any(|p| p.name == name) {
            bail!("a plugin named `{name}` is already loaded");
        }
        let mut plugin = Loaded {
            owner: self.native.plugins.len() as SystemOwner + 1,
            name,
            source,
            library: None,
            copy: None,
            current: None,
            settling: None,
            rejected: None,
            components: Vec::new(),
            generation: 0,
            keep_loaded: false,
        };
        install(self, &mut plugin)?;
        self.native.plugins.push(plugin);
        Ok(self)
    }

    pub fn native_plugins(&mut self) -> &mut NativePlugins {
        &mut self.native
    }

    /// Reloads every plugin whose file has changed, right now, without waiting for the file
    /// to settle. Returns how many were reloaded. The frame loop does this by itself (with
    /// the wait); call it when you know a build has just finished.
    pub fn reload_native_plugins(&mut self) -> usize {
        let mut plugins = std::mem::take(&mut self.native.plugins);
        let mut reloaded = 0;
        for plugin in &mut plugins {
            let now = stamp(&plugin.source);
            if now.is_some() && now != plugin.current && try_reload(self, plugin, now) {
                reloaded += 1;
            }
        }
        self.native.plugins = plugins;
        reloaded
    }

    /// Called once a frame.
    pub(crate) fn check_native_plugins(&mut self) {
        let host = &mut self.native;
        if !host.hot_reload || host.plugins.is_empty() {
            return;
        }
        if host
            .last_check
            .is_some_and(|t| t.elapsed() < host.check_interval)
        {
            return;
        }
        host.last_check = Some(Instant::now());

        let mut plugins = std::mem::take(&mut host.plugins);
        for plugin in &mut plugins {
            let now = stamp(&plugin.source);
            if now.is_none() || now == plugin.current || now == plugin.rejected {
                plugin.settling = None;
            } else if now == plugin.settling {
                // Unchanged since the last check: the compiler has finished writing it.
                plugin.settling = None;
                try_reload(self, plugin, now);
            } else {
                plugin.settling = now;
            }
        }
        self.native.plugins = plugins;
    }
}

fn try_reload(app: &mut App, plugin: &mut Loaded, version: Option<Stamp>) -> bool {
    match install(app, plugin) {
        Ok(()) => {
            log::info!(
                "reloaded plugin `{}` (load {})",
                plugin.name,
                plugin.generation
            );
            true
        }
        Err(err) => {
            log::error!("could not reload plugin `{}`: {err:#}", plugin.name);
            plugin.rejected = version;
            false
        }
    }
}

/// Loads the current version of the plugin's file, replacing whatever version was loaded.
/// On an error before the new library has been opened and checked, the old version is left
/// running untouched.
fn install(app: &mut App, plugin: &mut Loaded) -> anyhow::Result<()> {
    let version =
        stamp(&plugin.source).with_context(|| format!("can't read {}", plugin.source.display()))?;

    let dir = std::env::temp_dir().join("voxl-plugins");
    std::fs::create_dir_all(&dir)?;
    static SWEEP: std::sync::Once = std::sync::Once::new();
    SWEEP.call_once(|| sweep_stale_copies(&dir));
    // Never reuse a name: overwriting a library that is loaded (even by another app in this
    // process) changes code that is running, and macOS kills the process for it.
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let copy = dir.join(format!(
        "{}-{}-{}{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed),
        plugin.name,
        std::env::consts::DLL_SUFFIX
    ));
    std::fs::copy(&plugin.source, &copy)?;
    let copy = TempCopy(Some(copy));

    // One plugin loads at a time, in the whole process. A plugin's load may start a language
    // runtime that every plugin in that language shares (GHC's, for one), and such a runtime
    // can't be started from two threads at once: the second caller is told it is already
    // running while the first is still starting it. Apps load their plugins one after another
    // anyway; this is for several apps in one process, as in the tests.
    static LOADING: Mutex<()> = Mutex::new(());
    let _loading = LOADING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // SAFETY: loading a library runs its initializers, and calling into it runs its code;
    // both are as safe as the plugin is. That is the trust a native plugin is given.
    let (library, load, flags) = unsafe {
        let library = Library::new(copy.0.as_ref().unwrap())
            .with_context(|| format!("can't open {}", plugin.source.display()))?;
        let abi = *library
            .get::<unsafe extern "C" fn() -> u32>(b"voxl_plugin_abi_version\0")
            .context("it doesn't export `voxl_plugin_abi_version`")?;
        let abi = abi();
        if abi != sys::VOXL_ABI_VERSION {
            bail!(
                "it was built for plugin interface version {abi}, and this engine speaks \
                 version {}",
                sys::VOXL_ABI_VERSION
            );
        }
        let load = *library
            .get::<LoadFn>(b"voxl_plugin_load\0")
            .context("it doesn't export `voxl_plugin_load`")?;
        let flags = library
            .get::<unsafe extern "C" fn() -> u32>(b"voxl_plugin_flags\0")
            .map_or(0, |flags| flags());
        (library, load, flags)
    };

    // From here on the old version is being replaced.
    let first_load = plugin.generation == 0;
    retire(app, plugin);
    plugin.keep_loaded = flags & sys::VOXL_PLUGIN_KEEP_LOADED != 0;
    plugin.generation += 1;
    plugin.current = Some(version);
    plugin.rejected = None;

    let mut registrar = Registrar {
        world: &mut app.world,
        states: &mut app.native.states,
        plugin: &plugin.name,
        systems: Vec::new(),
        components: Vec::new(),
    };
    // SAFETY: as above. The registrar outlives the call, and the plugin may not keep it.
    let status = unsafe { load(&api::API, (&raw mut registrar).cast()) };
    let Registrar {
        systems,
        components,
        ..
    } = registrar;

    if status != 0 {
        // Its systems are dropped unrun; make sure nothing calls into the library again.
        for component in components {
            app.world.forget_blob_drop(component);
        }
        for stage in STAGES {
            app.schedule_mut(stage)
                .replace_owned(plugin.owner, Vec::new());
        }
        if plugin.keep_loaded {
            std::mem::forget(library);
        }
        bail!("its load function failed (returned {status})");
    }

    let mut by_stage: HashMap<Stage, Vec<BoxedSystem>> = HashMap::new();
    for (stage, system) in systems {
        by_stage.entry(stage).or_default().push(Box::new(system));
    }
    for stage in STAGES {
        let mut systems = by_stage.remove(&stage).unwrap_or_default();
        if stage == Stage::Startup && app.is_started() {
            // The app's startup has been and gone. A plugin arriving late gets its own, once;
            // a reload doesn't get another.
            if first_load {
                for system in &mut systems {
                    system.initialize(&mut app.world);
                    system.run(&mut app.world);
                }
            }
            systems.clear();
        }
        app.schedule_mut(stage).replace_owned(plugin.owner, systems);
    }

    plugin.components = components;
    plugin.library = Some(library);
    plugin.copy = Some(copy.keep());
    Ok(())
}

/// Tells the loaded version it is going away and closes it. Its systems stay in the
/// schedules until the caller replaces them, so nothing may run in between.
fn retire(app: &mut App, plugin: &mut Loaded) {
    let Some(library) = &plugin.library else {
        return;
    };
    // The destructors of its components are about to disappear with it. Until the next
    // version registers new ones, values are leaked rather than destroyed.
    for &component in &plugin.components {
        app.world.forget_blob_drop(component);
    }
    // SAFETY: calling into the plugin, as trusted as when it was loaded.
    unsafe {
        if let Ok(unload) = library.get::<unsafe extern "C" fn()>(b"voxl_plugin_unload\0") {
            unload();
        }
    }
    plugin.close();
}
