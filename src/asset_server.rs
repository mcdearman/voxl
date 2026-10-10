//! Assets by name.
//!
//! The `AssetServer` gives every asset it loads a name: the path of a file, optionally with a
//! note of how to read it (`bricks.png?linear`), the path of a file and a part of it
//! (`house.glb#mesh2`), or a shape made in code (`shape:cube:1`). Asking for a name twice
//! gives the same handle.
//!
//! Names are what make a saved scene mean the same thing later. A handle is only a number,
//! valid while the app runs; a scene saves the name instead, and loading the scene asks the
//! server for it again.
//!
//! Images are decoded on worker threads. The handle comes back at once and the image appears
//! under it a moment later; until then anything using it draws with a plain white texture.
//! Image files are also watched: save one again and it is reloaded in place.

use std::{
    any::TypeId,
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;

use crate::{
    assets::{Assets, Handle},
    ecs::{ResMut, World},
    reflect::{TypeRegistry, Value},
    render::{Animator, GltfScene, Image, Mesh, Playing, Skinned},
    tasks::{Mailbox, TaskPool},
};

/// A file's modification time and size: enough to tell that it was saved again.
pub(crate) type Stamp = (SystemTime, u64);

pub(crate) fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// For each asset, by type and id: what scene files call its kind, and its name.
type Names = HashMap<(TypeId, u32), (&'static str, String)>;

/// Loads the asset of one kind with the given name, returning its id.
pub type Resolver = fn(&mut AssetServer, &mut World, &str) -> anyhow::Result<u32>;

struct WatchedImage {
    id: u32,
    path: PathBuf,
    srgb: bool,
    /// The version of the file that is loaded, or being loaded.
    current: Option<Stamp>,
    /// A newer version seen on the last check, waiting to see whether it is still changing.
    settling: Option<Stamp>,
}

/// A model file being watched for changes.
struct WatchedModel {
    name: String,
    path: PathBuf,
    current: Option<Stamp>,
    settling: Option<Stamp>,
}

/// A model file read on a worker, on its way back.
struct ImportedModel {
    name: String,
    imported: anyhow::Result<crate::render::Imported>,
}

/// How a picture file is to be made ready, as a worker needs to know it.
struct Processing {
    compress: bool,
    keep_pixels: bool,
    cache: Option<PathBuf>,
    limit: u64,
}

impl Processing {
    /// The picture in a file's bytes: straight from the cache if these bytes were processed
    /// before, else decoded, and compressed and kept if that is wanted.
    fn image(&self, bytes: &[u8], srgb: bool) -> anyhow::Result<Image> {
        if !self.compress {
            return Image::from_bytes(bytes, srgb);
        }
        let key = crate::asset_cache::key(bytes, if srgb { "bc7-srgb" } else { "bc7-linear" });
        let cached = self.cache.as_ref().and_then(|dir| crate::asset_cache::read(dir, &key));
        if let (Some(processed), false) = (&cached, self.keep_pixels) {
            return Ok(Image::from_processed(processed.clone()));
        }
        Ok(self.finish(Image::from_bytes(bytes, srgb)?, &key, cached))
    }

    /// Compresses a picture that is here as pixels, keeping the result under `key`; or takes
    /// what the cache already had for it.
    fn finish(&self, mut image: Image, key: &str, cached: Option<crate::render::Processed>) -> Image {
        let Some(processed) = cached.or_else(|| {
            let processed = image.process()?;
            if let Some(dir) = &self.cache {
                crate::asset_cache::write(dir, key, &processed);
                crate::asset_cache::prune(dir, self.limit);
            }
            Some(processed)
        }) else {
            // A size blocks can't hold: it stays as plain pixels.
            return image;
        };
        if !self.keep_pixels {
            image.data = Vec::new();
        }
        image.processed = Some(Arc::new(processed));
        image
    }

    /// The same for a picture that came as pixels, from inside a model file: named in the
    /// cache by its pixels.
    fn pixels(&self, image: Image) -> Image {
        if !self.compress {
            return image;
        }
        let how = format!("bc7-{}-pixels-{}x{}", if image.srgb { "srgb" } else { "linear" }, image.width, image.height);
        let key = crate::asset_cache::key(&image.data, &how);
        let cached = self.cache.as_ref().and_then(|dir| crate::asset_cache::read(dir, &key));
        self.finish(image, &key, cached)
    }
}

struct DecodedImage {
    id: u32,
    path: PathBuf,
    image: anyhow::Result<Image>,
}

/// How far along an asset is, by name. See [`AssetServer::state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetState {
    /// Nothing has asked for an asset by this name.
    Unknown,
    /// Asked for, and being read or decoded on a worker.
    Loading,
    Loaded,
    /// Asked for, and its file could not be read. It is tried again when the file changes.
    Failed,
}

/// Knows every loaded asset by name. A resource.
pub struct AssetServer {
    root: PathBuf,
    names: Names,
    ids: HashMap<(TypeId, String), u32>,
    resolvers: HashMap<&'static str, Resolver>,
    /// Assets `unload_unused` leaves alone whether or not anything is seen to use them.
    kept: HashSet<(TypeId, u32)>,
    gltf: HashMap<String, Arc<GltfScene>>,
    pool: Option<TaskPool>,
    decoded: Mailbox<DecodedImage>,
    loading: usize,
    watched: Vec<WatchedImage>,
    /// Model files being read on workers, those that could not be read, those watched for
    /// changes, and those whose scene has just arrived (for the first time, or anew).
    imported: Mailbox<ImportedModel>,
    models_loading: HashSet<String>,
    models_failed: HashSet<String>,
    watched_models: Vec<WatchedModel>,
    arrived: Vec<String>,
    /// The images being decoded, and those whose files could not be.
    images_loading: HashSet<u32>,
    images_failed: HashSet<u32>,
    /// What each asset needs, by name: see [`AssetServer::depends_on`].
    needs: HashMap<String, BTreeSet<String>>,
    last_check: Option<Instant>,
    /// Whether pictures are compressed into blocks for the graphics card as they are loaded
    /// (see [`asset_cache`](crate::asset_cache)). The renderer turns this on where the card
    /// can hold them; it is off where there is no renderer.
    pub compress: bool,
    /// Whether a compressed picture also keeps its pixels, for a game that reads them.
    pub keep_pixels: bool,
    /// Whether what is processed is kept on disk, in `.mira/cache` under the root.
    pub cache: bool,
    /// The most the cache may hold, in bytes, before what was used longest ago goes.
    pub cache_limit: u64,
    /// How often image files are checked for changes.
    pub check_interval: Duration,
    /// Whether changed image files are reloaded. On in development builds.
    pub hot_reload: bool,
}

impl Default for AssetServer {
    fn default() -> Self {
        Self::new(".")
    }
}

/// A file name without the `?how` or `#part` after it.
fn file_part(name: &str) -> &str {
    name.split(['?', '#']).next().unwrap_or(name)
}

impl AssetServer {
    /// A server whose file names are relative to `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut resolvers: HashMap<&'static str, Resolver> = HashMap::new();
        resolvers.insert("image", |server, world, name| {
            let images = world
                .get_resource_mut::<Assets<Image>>()
                .context("this app has no image assets")?;
            Ok(server.load_image(images, name).id())
        });
        resolvers.insert("mesh", |server, world, name| {
            if name.starts_with("shape:") {
                let meshes = world
                    .get_resource_mut::<Assets<Mesh>>()
                    .context("this app has no mesh assets")?;
                return Ok(server.shape(meshes, name)?.id());
            }
            // A part of a model file: load the file, which names all of its parts.
            if name.contains('#') {
                world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                    world.resource_scope(|_, images: &mut Assets<Image>| {
                        server.load_gltf(file_part(name), meshes, images)
                    })
                })?;
            }
            server
                .find::<Mesh>(name)
                .map(|handle| handle.id())
                .with_context(|| format!("there is no mesh named `{name}`"))
        });
        Self {
            root: root.into(),
            names: HashMap::new(),
            ids: HashMap::new(),
            resolvers,
            kept: HashSet::new(),
            gltf: HashMap::new(),
            pool: None,
            decoded: Mailbox::default(),
            loading: 0,
            watched: Vec::new(),
            imported: Mailbox::default(),
            models_loading: HashSet::new(),
            models_failed: HashSet::new(),
            watched_models: Vec::new(),
            arrived: Vec::new(),
            images_loading: HashSet::new(),
            images_failed: HashSet::new(),
            needs: HashMap::new(),
            last_check: None,
            compress: false,
            keep_pixels: false,
            cache: true,
            cache_limit: 2 << 30,
            check_interval: Duration::from_millis(250),
            hot_reload: cfg!(debug_assertions),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_root(&mut self, root: impl Into<PathBuf>) {
        self.root = root.into();
    }

    /// Where the file behind a name is.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(file_part(name))
    }

    /// Makes a handle findable under a name, and so savable in scenes. `kind` is what scene
    /// files call this type of asset (`"image"`, `"mesh"`).
    pub fn name<T: 'static>(
        &mut self,
        kind: &'static str,
        handle: Handle<T>,
        name: impl Into<String>,
    ) {
        let name = name.into();
        let key = TypeId::of::<T>();
        self.ids.insert((key, name.clone()), handle.id());
        self.names.insert((key, handle.id()), (kind, name));
    }

    /// The name a handle was loaded or registered under.
    pub fn name_of<T: 'static>(&self, handle: Handle<T>) -> Option<&str> {
        self.names
            .get(&(TypeId::of::<T>(), handle.id()))
            .map(|(_, name)| name.as_str())
    }

    /// The handle for a name, if that asset has been asked for before.
    pub fn find<T: 'static>(&self, name: &str) -> Option<Handle<T>> {
        self.ids
            .get(&(TypeId::of::<T>(), name.to_owned()))
            .map(|&id| Handle::from_id(id))
    }

    /// Teaches the server to load another kind of asset by name, for scenes.
    pub fn add_resolver(&mut self, kind: &'static str, resolver: Resolver) {
        self.resolvers.insert(kind, resolver);
    }

    /// Loads an asset knowing only its kind and name, as a scene file gives them.
    pub fn resolve(world: &mut World, kind: &str, name: &str) -> anyhow::Result<u32> {
        if !world.contains_resource::<AssetServer>() {
            anyhow::bail!("this app has no asset server");
        }
        world.resource_scope(|world, server: &mut AssetServer| {
            let resolver = *server
                .resolvers
                .get(kind)
                .with_context(|| format!("nothing knows how to load a `{kind}`"))?;
            resolver(server, world, name)
        })
    }

    // --- what needs what ---

    /// Records that the asset called `name` needs the one called `on`: a model its meshes
    /// and textures, a prefab the models and pictures its entities use and the prefabs
    /// inside it. The server records these for what it loads itself; a game that loads a
    /// kind of its own says so here. Needing something keeps it from being unloaded while
    /// what needs it is in use.
    pub fn depends_on(&mut self, name: &str, on: &str) {
        if name != on {
            self.needs.entry(name.to_owned()).or_default().insert(on.to_owned());
        }
    }

    /// Says everything `name` needs at once, in place of what it was said to need before:
    /// for an asset that has been loaded again and may have changed.
    pub fn set_dependencies(&mut self, name: &str, on: impl IntoIterator<Item = String>) {
        let on: BTreeSet<String> = on.into_iter().filter(|other| other != name).collect();
        if on.is_empty() {
            self.needs.remove(name);
        } else {
            self.needs.insert(name.to_owned(), on);
        }
    }

    /// What `name` needs directly, in order of name.
    pub fn dependencies(&self, name: &str) -> Vec<&str> {
        self.needs
            .get(name)
            .map_or(Vec::new(), |on| on.iter().map(String::as_str).collect())
    }

    /// Everything `name` needs, and what those need, and so on; in order of name, and
    /// without `name` itself. Assets may need each other in a ring.
    pub fn all_dependencies(&self, name: &str) -> Vec<String> {
        let mut found = self.reach([name.to_owned()]);
        found.remove(name);
        found.into_iter().collect()
    }

    /// What needs `name` directly, in order of name.
    pub fn dependents(&self, name: &str) -> Vec<&str> {
        let mut found: Vec<&str> = self
            .needs
            .iter()
            .filter(|(_, on)| on.contains(name))
            .map(|(other, _)| other.as_str())
            .collect();
        found.sort_unstable();
        found
    }

    /// The names given, and everything they need, however far down.
    fn reach(&self, from: impl IntoIterator<Item = String>) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut next: Vec<String> = from.into_iter().collect();
        while let Some(name) = next.pop() {
            if let Some(on) = self.needs.get(&name) {
                next.extend(on.iter().filter(|other| !found.contains(*other)).cloned());
            }
            found.insert(name);
        }
        found
    }

    /// How far along the asset called `name` is, not counting what it needs.
    pub fn state(&self, name: &str) -> AssetState {
        if self.models_loading.contains(name) {
            return AssetState::Loading;
        }
        if self.models_failed.contains(name) {
            return AssetState::Failed;
        }
        if self.gltf.contains_key(name) {
            return AssetState::Loaded;
        }
        let ids: Vec<(TypeId, u32)> = self
            .ids
            .iter()
            .filter(|((_, known), _)| known == name)
            .map(|((kind, _), id)| (*kind, *id))
            .collect();
        if ids.is_empty() {
            // Known only as something another asset needs, or as a name others are told of.
            return if self.needs.contains_key(name) { AssetState::Loaded } else { AssetState::Unknown };
        }
        let image = TypeId::of::<Image>();
        if ids.iter().any(|(kind, id)| *kind == image && self.images_loading.contains(id)) {
            AssetState::Loading
        } else if ids.iter().any(|(kind, id)| *kind == image && self.images_failed.contains(id)) {
            AssetState::Failed
        } else {
            AssetState::Loaded
        }
    }

    /// Whether `name` and everything it needs is loaded: the moment a level can be shown
    /// with nothing still to pop in.
    pub fn is_ready(&self, name: &str) -> bool {
        self.reach([name.to_owned()])
            .iter()
            .all(|needed| self.state(needed) == AssetState::Loaded)
    }

    /// Every name the server knows, in order: assets loaded or loading, and whatever they
    /// are said to need.
    pub fn known(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = self.names.values().map(|(_, name)| name.clone()).collect();
        names.extend(self.gltf.keys().cloned());
        names.extend(self.models_loading.iter().cloned());
        names.extend(self.models_failed.iter().cloned());
        for (name, on) in &self.needs {
            names.insert(name.clone());
            names.extend(on.iter().cloned());
        }
        names.into_iter().collect()
    }

    // --- unloading ---

    /// Protects an asset from [`AssetServer::unload_unused`]. For handles held where the
    /// engine can't see them: in a resource, or in a component that isn't registered.
    pub fn keep<T: 'static>(&mut self, handle: Handle<T>) {
        self.kept.insert((TypeId::of::<T>(), handle.id()));
    }

    /// Undoes [`AssetServer::keep`].
    pub fn release<T: 'static>(&mut self, handle: Handle<T>) {
        self.kept.remove(&(TypeId::of::<T>(), handle.id()));
    }

    /// Unloads the named images and meshes that nothing uses, freeing their memory on the GPU
    /// too, and returns how many went. Call it at a quiet moment: after changing level, say.
    ///
    /// An asset is in use if a registered component on some entity refers to it, if it was
    /// [kept](AssetServer::keep), or if it is part of a model file another part of which is
    /// in use. Handles held anywhere else go stale, so keep those. Nothing is lost for good:
    /// asking for an unloaded name loads it again, which is what spawning a scene does.
    /// Assets without a name are never touched.
    pub fn unload_unused(world: &mut World) -> usize {
        if !world.contains_resource::<AssetServer>() || !world.contains_resource::<TypeRegistry>() {
            return 0;
        }
        world.resource_scope(|world, server: &mut AssetServer| {
            let mut used: HashSet<(TypeId, u32)> = server.kept.clone();
            // A skinned mesh draws with a copy, and refers to the original from a component
            // that can't be saved.
            for skinned in world.query::<&Skinned>().iter() {
                used.insert((TypeId::of::<Mesh>(), skinned.source.id()));
            }
            let mut named: HashSet<(String, String)> = HashSet::new();
            let registry = world.resource::<TypeRegistry>();
            with_names(server, || {
                for component in registry.iter() {
                    for entity in (component.entities)(world) {
                        let Some(mut value) = (component.get)(world, entity) else {
                            continue;
                        };
                        value.for_each_asset(&mut |asset| {
                            if let Value::Asset { kind, name } = asset {
                                named.insert((kind.clone(), name.clone()));
                            }
                        });
                    }
                }
            });
            // What is used by name: what components refer to, the model files and prefabs
            // entities are made from, what is kept; and everything those need.
            let mut roots: Vec<String> = named.iter().map(|(_, name)| name.clone()).collect();
            roots.extend(
                server
                    .names
                    .iter()
                    .filter(|(key, _)| used.contains(key))
                    .map(|(_, (_, name))| name.clone()),
            );
            roots.extend(world.query::<&Model>().iter().map(|model| model.name.clone()));
            roots.extend(
                world
                    .query::<&crate::prefab::PrefabInstance>()
                    .iter()
                    .map(|instance| instance.prefab.clone()),
            );
            let needed = server.reach(roots);
            let unused: Vec<(TypeId, u32, String)> = server
                .names
                .iter()
                .filter(|(key, (_, name))| !used.contains(key) && !needed.contains(name))
                .map(|(&(asset_type, id), (_, name))| (asset_type, id, name.clone()))
                .collect();

            let mut unloaded = 0;
            for (asset_type, id, name) in unused {
                if asset_type == TypeId::of::<Image>() {
                    if let Some(images) = world.get_resource_mut::<Assets<Image>>() {
                        images.remove(Handle::from_id(id));
                    }
                    server.watched.retain(|watched| watched.id != id);
                } else if asset_type == TypeId::of::<Mesh>() {
                    if let Some(meshes) = world.get_resource_mut::<Assets<Mesh>>() {
                        meshes.remove(Handle::from_id(id));
                    }
                } else {
                    // Some other kind of asset, kept somewhere this doesn't know about.
                    continue;
                }
                server.names.remove(&(asset_type, id));
                server.needs.remove(&name);
                server.ids.remove(&(asset_type, name));
                unloaded += 1;
            }
            // A model file that went needs nothing any more.
            let dropped: Vec<String> = server
                .gltf
                .keys()
                .filter(|file| !needed.contains(*file))
                .cloned()
                .collect();
            for file in dropped {
                server.gltf.remove(&file);
                server.needs.remove(&file);
            }
            if unloaded > 0 {
                log::info!("unloaded {unloaded} unused assets");
            }
            unloaded
        })
    }

    // --- images ---

    /// Loads a PNG or JPEG as a colour texture. Add `?linear` to the name for data such as
    /// normal and roughness maps. The handle is returned at once; the image arrives under it
    /// when it has been decoded, and again whenever the file changes.
    pub fn load_image(&mut self, images: &mut Assets<Image>, name: &str) -> Handle<Image> {
        if let Some(handle) = self.find::<Image>(name) {
            return handle;
        }
        let handle = images.reserve();
        self.name("image", handle, name);
        let path = self.path(name);
        let srgb = !name.ends_with("?linear");
        self.watched.push(WatchedImage {
            id: handle.id(),
            current: stamp(&path),
            settling: None,
            path: path.clone(),
            srgb,
        });
        self.decode(handle.id(), path, srgb);
        handle
    }

    fn decode(&mut self, id: u32, path: PathBuf, srgb: bool) {
        self.loading += 1;
        self.images_loading.insert(id);
        self.images_failed.remove(&id);
        let how = self.processing();
        let pool = self.pool.get_or_insert_with(|| TaskPool::new(2));
        pool.spawn(&self.decoded.sender, move || {
            let image = std::fs::read(&path)
                .with_context(|| format!("can't read {}", path.display()))
                .and_then(|bytes| {
                    how.image(&bytes, srgb)
                        .with_context(|| format!("can't decode {}", path.display()))
                });
            DecodedImage { id, path, image }
        });
    }

    fn processing(&self) -> Processing {
        Processing {
            compress: self.compress,
            keep_pixels: self.keep_pixels,
            cache: self.cache.then(|| self.cache_dir()),
            limit: self.cache_limit,
        }
    }

    /// Where processed assets are kept.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(".mira").join("cache")
    }

    /// How many assets are still being loaded.
    pub fn loading(&self) -> usize {
        self.loading
    }

    fn accept(&mut self, images: &mut Assets<Image>, decoded: DecodedImage) {
        self.loading -= 1;
        self.images_loading.remove(&decoded.id);
        // Unloaded while it was being decoded: nothing wants it any more.
        if !self
            .names
            .contains_key(&(TypeId::of::<Image>(), decoded.id))
        {
            return;
        }
        match decoded.image {
            Ok(image) => {
                let reloaded = images.contains_id(decoded.id);
                images.set(Handle::from_id(decoded.id), image);
                if reloaded {
                    log::info!("reloaded {}", decoded.path.display());
                }
            }
            // The handle stays empty (or keeps the old image), which draws as plain white.
            Err(err) => {
                log::error!("{err:#}");
                if !images.contains_id(decoded.id) {
                    self.images_failed.insert(decoded.id);
                }
            }
        }
    }

    /// Takes in the images that have finished decoding. Returns how many arrived.
    pub fn finish(&mut self, images: &mut Assets<Image>) -> usize {
        let mut arrived = 0;
        while let Ok(decoded) = self.decoded.try_recv() {
            self.accept(images, decoded);
            arrived += 1;
        }
        arrived
    }

    /// Waits until nothing is still loading. For tools and tests; a game should carry on and
    /// let assets arrive.
    pub fn wait(&mut self, images: &mut Assets<Image>) {
        while self.loading > 0 {
            match self.decoded.recv() {
                Ok(decoded) => self.accept(images, decoded),
                Err(_) => break,
            }
        }
    }

    /// Reloads every image whose file has changed, right now, without waiting for the file
    /// to settle. Returns how many reloads were started.
    pub fn reload_changed(&mut self) -> usize {
        let changed: Vec<usize> = (0..self.watched.len())
            .filter(|&i| {
                let now = stamp(&self.watched[i].path);
                now.is_some() && now != self.watched[i].current
            })
            .collect();
        for &i in &changed {
            self.reload(i);
        }
        let models: Vec<usize> = (0..self.watched_models.len())
            .filter(|&i| {
                let now = stamp(&self.watched_models[i].path);
                now.is_some() && now != self.watched_models[i].current
            })
            .collect();
        for &i in &models {
            self.reload_model(i);
        }
        changed.len() + models.len()
    }

    fn reload_model(&mut self, index: usize) {
        let watched = &mut self.watched_models[index];
        watched.current = stamp(&watched.path);
        watched.settling = None;
        let name = watched.name.clone();
        self.models_failed.remove(&name);
        if !self.models_loading.contains(&name) {
            self.import(&name);
        }
    }

    fn reload(&mut self, index: usize) {
        let watched = &mut self.watched[index];
        watched.current = stamp(&watched.path);
        watched.settling = None;
        let (id, path, srgb) = (watched.id, watched.path.clone(), watched.srgb);
        self.decode(id, path, srgb);
    }

    fn check_files(&mut self) {
        if !self.hot_reload || (self.watched.is_empty() && self.watched_models.is_empty()) {
            return;
        }
        if self
            .last_check
            .is_some_and(|t| t.elapsed() < self.check_interval)
        {
            return;
        }
        self.last_check = Some(Instant::now());
        for index in 0..self.watched.len() {
            let watched = &mut self.watched[index];
            let now = stamp(&watched.path);
            if now.is_none() || now == watched.current {
                watched.settling = None;
            } else if now == watched.settling {
                // Unchanged since the last check: whatever was writing it has finished.
                self.reload(index);
            } else {
                watched.settling = now;
            }
        }
        for index in 0..self.watched_models.len() {
            let watched = &mut self.watched_models[index];
            let now = stamp(&watched.path);
            if now.is_none() || now == watched.current {
                watched.settling = None;
            } else if now == watched.settling {
                self.reload_model(index);
            } else {
                watched.settling = now;
            }
        }
    }

    // --- meshes ---

    /// A mesh made in code, by name: `shape:cube:<edge>`, `shape:sphere:<radius>` or
    /// `shape:plane:<edge>`. The same name always gives the same handle, and because it has a
    /// name, entities drawn with it can be saved in a scene.
    pub fn shape(&mut self, meshes: &mut Assets<Mesh>, name: &str) -> anyhow::Result<Handle<Mesh>> {
        if let Some(handle) = self.find::<Mesh>(name) {
            return Ok(handle);
        }
        let mut parts = name.split(':');
        let (Some("shape"), Some(shape), Some(size), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            anyhow::bail!("`{name}` isn't of the form `shape:cube:1`");
        };
        let size: f32 = size
            .parse()
            .ok()
            .filter(|size: &f32| size.is_finite() && *size > 0.0)
            .with_context(|| format!("`{name}` needs a size greater than zero"))?;
        let mesh = match shape {
            "cube" => Mesh::cube(size),
            "sphere" => Mesh::uv_sphere(size, 32, 16),
            "plane" => Mesh::plane(size),
            other => anyhow::bail!("there is no shape called `{other}`"),
        };
        let handle = meshes.add(mesh);
        self.name("mesh", handle, name);
        Ok(handle)
    }

    pub fn cube(&mut self, meshes: &mut Assets<Mesh>, size: f32) -> Handle<Mesh> {
        self.shape(meshes, &format!("shape:cube:{size}"))
            .expect("a cube needs a size greater than zero")
    }

    pub fn sphere(&mut self, meshes: &mut Assets<Mesh>, radius: f32) -> Handle<Mesh> {
        self.shape(meshes, &format!("shape:sphere:{radius}"))
            .expect("a sphere needs a radius greater than zero")
    }

    pub fn plane(&mut self, meshes: &mut Assets<Mesh>, size: f32) -> Handle<Mesh> {
        self.shape(meshes, &format!("shape:plane:{size}"))
            .expect("a plane needs a size greater than zero")
    }

    // --- models ---

    /// Loads a glTF file (`.gltf` or `.glb`), once. Its meshes are named `file#mesh0`,
    /// `file#mesh1`, … and its textures `file#image0`, … in the order the file gives them, so
    /// entities using them can be saved in a scene and found again.
    pub fn load_gltf(
        &mut self,
        name: &str,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
    ) -> anyhow::Result<Arc<GltfScene>> {
        if let Some(scene) = self.gltf.get(name) {
            return Ok(scene.clone());
        }
        let imported = GltfScene::import(self.path(name))?;
        self.adopt(name, imported, meshes, images)
    }

    /// Makes a read model file the scene known by `name`, in place of any there was, and
    /// names its meshes and images.
    fn adopt(
        &mut self,
        name: &str,
        imported: crate::render::Imported,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
    ) -> anyhow::Result<Arc<GltfScene>> {
        let (first_mesh, first_image) = (meshes.next_id(), images.next_id());
        let scene = Arc::new(GltfScene::build(imported, meshes, images)?);
        // Whatever the loader added is this file's, in an order that depends only on the file.
        let mut parts = Vec::new();
        for id in first_mesh..meshes.next_id() {
            let part = format!("{name}#mesh{}", id - first_mesh);
            self.name("mesh", Handle::<Mesh>::from_id(id), part.clone());
            parts.push(part);
        }
        for id in first_image..images.next_id() {
            let part = format!("{name}#image{}", id - first_image);
            self.name("image", Handle::<Image>::from_id(id), part.clone());
            parts.push(part);
        }
        // The file needs its parts and each part the file: a model stays or goes whole.
        for part in &parts {
            self.depends_on(part, name);
        }
        self.set_dependencies(name, parts);
        self.gltf.insert(name.to_owned(), scene.clone());
        if !self
            .watched_models
            .iter()
            .any(|watched| watched.name == name)
        {
            let path = self.path(name);
            self.watched_models.push(WatchedModel {
                name: name.to_owned(),
                current: stamp(&path),
                settling: None,
                path,
            });
        }
        Ok(scene)
    }

    /// The scene of a model file, if it has been loaded.
    pub fn model(&self, name: &str) -> Option<Arc<GltfScene>> {
        self.gltf.get(name).cloned()
    }

    /// Asks for a model file to be loaded without waiting for it: the file is read and
    /// decoded on a worker, and its scene is there (see [`model`](Self::model)) some frames
    /// later. Returns whether it is there already. A file that could not be read is not
    /// tried again until it changes.
    pub fn request_gltf(&mut self, name: &str) -> bool {
        if self.gltf.contains_key(name) {
            return true;
        }
        if !self.models_loading.contains(name) && !self.models_failed.contains(name) {
            self.import(name);
        }
        false
    }

    fn import(&mut self, name: &str) {
        self.models_loading.insert(name.to_owned());
        self.loading += 1;
        let (name, path) = (name.to_owned(), self.path(name));
        let how = self.processing();
        let pool = self.pool.get_or_insert_with(|| TaskPool::new(2));
        pool.spawn(&self.imported.sender, move || ImportedModel {
            // Its textures are compressed here too, where the file is read.
            imported: GltfScene::import(&path).map(|mut imported| {
                imported.prepare(|image| how.pixels(image));
                imported
            }),
            name,
        });
    }

    /// Takes in the model files that have been read, making their meshes and images.
    /// Returns how many arrived.
    pub fn finish_models(
        &mut self,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
    ) -> usize {
        let mut arrived = 0;
        while let Ok(model) = self.imported.try_recv() {
            arrived += 1;
            self.loading -= 1;
            self.models_loading.remove(&model.name);
            let reloading = self.gltf.contains_key(&model.name);
            let made = model
                .imported
                .and_then(|imported| self.adopt(&model.name, imported, meshes, images));
            match made {
                Ok(_) => {
                    if reloading {
                        log::info!("reloaded {}", model.name);
                    }
                    self.arrived.push(model.name);
                }
                // What was loaded before, if anything, stays as it is.
                Err(err) => {
                    log::error!("{err:#}");
                    self.models_failed.insert(model.name);
                }
            }
        }
        arrived
    }

    /// Waits until the model files asked for have arrived. For tools and tests.
    pub fn wait_for_models(&mut self, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) {
        while !self.models_loading.is_empty() {
            self.finish_models(meshes, images);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The model files whose scenes have arrived since this was last asked: loaded for the
    /// first time, or loaded again because the file changed.
    pub fn take_arrived_models(&mut self) -> Vec<String> {
        std::mem::take(&mut self.arrived)
    }
}

thread_local! {
    /// The names of the assets being saved, while a scene is captured on this thread.
    static NAMES: RefCell<Option<Names>> = const { RefCell::new(None) };
}

/// Runs `capture` with the server's names available to every handle it turns into a value,
/// so that handles are saved by name.
pub(crate) fn with_names<R>(server: &AssetServer, capture: impl FnOnce() -> R) -> R {
    NAMES.with(|names| *names.borrow_mut() = Some(server.names.clone()));
    let result = capture();
    NAMES.with(|names| *names.borrow_mut() = None);
    result
}

/// The kind and name of an asset, if a scene is being captured and the server knows it.
pub(crate) fn name_in_scope(asset_type: TypeId, id: u32) -> Option<(&'static str, String)> {
    NAMES.with(|names| names.borrow().as_ref()?.get(&(asset_type, id)).cloned())
}

/// Takes in finished loads and checks watched files, once a frame.
pub(crate) fn update_asset_server(
    mut server: ResMut<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if server.loading > 0 {
        server.finish(&mut images);
        server.finish_models(&mut meshes, &mut images);
    }
    server.check_files();
}

/// A model file, shown where this entity is: the engine reads the file without holding the
/// game up, and puts each part of the model under the entity when it arrives, and again
/// whenever the file is saved. Saved in a scene by the file's name alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, crate::reflect::Reflect)]
#[reflect(name = "mira.Model")]
pub struct Model {
    /// The file, by the name the asset server knows it by.
    pub name: String,
}

impl crate::ecs::Component for Model {}

impl Model {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// On an entity with a [`Model`]: which file's parts are under it now.
pub struct ModelShown(String);

impl crate::ecs::Component for ModelShown {}

/// On each part of a model that the engine put under a [`Model`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelPart;

impl crate::ecs::Component for ModelPart {}

/// Puts the parts of each [`Model`] under its entity once its file has been read, and puts
/// them there afresh when the file, or which file it is, has changed.
pub(crate) fn show_models(world: &mut World) {
    use crate::{
        reflect::NotSaved,
        render::Mesh3d,
        transform::{Parent, Transform},
    };
    if !world.contains_resource::<AssetServer>() {
        return;
    }
    let arrived = world.resource_mut::<AssetServer>().take_arrived_models();
    let wanted: Vec<(crate::ecs::Entity, String, bool)> = world
        .query::<(crate::ecs::Entity, &Model, Option<&ModelShown>)>()
        .iter()
        .map(|(entity, model, shown)| {
            let fresh =
                shown.is_some_and(|shown| shown.0 == model.name) && !arrived.contains(&model.name);
            (entity, model.name.clone(), fresh)
        })
        .collect();
    for (entity, name, fresh) in wanted {
        if fresh || name.is_empty() {
            continue;
        }
        let server = world.resource_mut::<AssetServer>();
        let Some(scene) = server.model(&name) else {
            server.request_gltf(&name);
            continue;
        };
        // Out with the parts that were there, in with this file's.
        let old: Vec<crate::ecs::Entity> = world
            .query::<(crate::ecs::Entity, &Parent, &ModelPart)>()
            .iter()
            .filter(|(_, parent, _)| parent.0 == entity)
            .map(|(part, _, _)| part)
            .collect();
        for part in old {
            // A part a skeleton bends draws with a mesh of its own, which goes with it.
            if let (Some(_), Some(mesh)) = (world.get::<Skinned>(part), world.get::<Mesh3d>(part).copied()) {
                if let Some(meshes) = world.get_resource_mut::<Assets<Mesh>>() {
                    meshes.remove(mesh.0);
                }
            }
            world.despawn(part);
        }
        // A model with a skeleton gets an animator, which its skinned parts follow.
        let palette = scene.skeleton.as_ref().map(|skeleton| {
            let animator = Animator::new(skeleton.clone(), scene.clips.clone());
            let palette = animator.palette.clone();
            world.insert(entity, (animator,));
            if let Some(playing) = world.get_mut::<Playing>(entity) {
                playing.restart();
            }
            palette
        });
        if palette.is_none() {
            world.remove::<Animator>(entity);
        }
        for part in &scene.parts {
            if let (Some(weights), Some(palette)) = (&part.skin, &palette) {
                let Some(meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                    continue;
                };
                let Some(copy) = meshes.get(part.mesh).cloned() else {
                    continue;
                };
                let mesh = meshes.add(copy);
                let skinned = Skinned {
                    source: part.mesh,
                    weights: weights.clone(),
                    palette: palette.clone(),
                };
                world.spawn((
                    Transform::IDENTITY,
                    Mesh3d(mesh),
                    part.material,
                    skinned,
                    Parent(entity),
                    ModelPart,
                    NotSaved,
                ));
                continue;
            }
            let (scale, rotation, translation) = part.transform.to_scale_rotation_translation();
            world.spawn((
                Transform {
                    translation,
                    rotation,
                    scale,
                },
                Mesh3d(part.mesh),
                part.material,
                Parent(entity),
                ModelPart,
                // Made again from the file whenever the scene is loaded.
                NotSaved,
            ));
        }
        world.insert(entity, ModelShown(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        reflect::{Scene, TypeRegistry},
        render::{Material, Mesh3d},
        transform::Transform,
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("mira-assets-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn png(&self, name: &str, rgba: [u8; 4]) {
            image::RgbaImage::from_pixel(2, 2, image::Rgba(rgba))
                .save(self.0.join(name))
                .unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pixel(images: &Assets<Image>, handle: Handle<Image>) -> Option<[u8; 4]> {
        images
            .get(handle)
            .map(|image| image.data[..4].try_into().unwrap())
    }

    #[test]
    fn a_model_is_read_on_a_worker_and_shown_again_when_its_file_changes() {
        use crate::{
            app::App,
            ecs::Entity,
            reflect::NotSaved,
            transform::{Parent, TransformPlugin},
        };
        let dir = TempDir::new("models");
        let hen =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("res/paris/models/animals/hen_white.glb");
        std::fs::copy(&hen, dir.0.join("bird.glb")).unwrap();

        let mut app = App::new();
        app.add_plugins(TransformPlugin)
            .insert_resource(AssetServer::new(&dir.0))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .add_systems(
                crate::app::Stage::PreUpdate,
                (update_asset_server, show_models),
            );
        let bird = app
            .world
            .spawn((Transform::IDENTITY, Model::new("bird.glb")));
        let parts = |app: &mut App| {
            app.world
                .query::<(Entity, &Parent, &ModelPart)>()
                .iter()
                .filter(|(_, parent, _)| parent.0 == bird)
                .map(|(part, _, _)| part)
                .collect::<Vec<_>>()
        };
        // Asked for, nothing is there at once and the game is not held up for it.
        app.update();
        assert!(parts(&mut app).is_empty() && app.world.resource::<AssetServer>().loading() == 1);
        let frames = |app: &mut App, until: &dyn Fn(&mut App) -> bool| {
            for _ in 0..3000 {
                app.update();
                if until(app) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("it did not arrive");
        };
        frames(&mut app, &|app| !parts(app).is_empty());
        let first = parts(&mut app);
        let expected = app
            .world
            .resource::<AssetServer>()
            .model("bird.glb")
            .unwrap()
            .parts
            .len();
        assert_eq!(first.len(), expected);
        assert!(first
            .iter()
            .all(|part| app.world.get::<NotSaved>(*part).is_some()));
        assert!(app
            .world
            .resource::<AssetServer>()
            .find::<Mesh>("bird.glb#mesh0")
            .is_some());
        // Left alone, it is left alone.
        app.update();
        assert_eq!(parts(&mut app), first);

        // The file saved again: it is read again and the parts are put there afresh.
        std::fs::copy(&hen, dir.0.join("bird.glb")).unwrap();
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.0.join("bird.glb"))
            .unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        assert_eq!(app.world.resource_mut::<AssetServer>().reload_changed(), 1);
        frames(&mut app, &|app| {
            parts(app) != first && !parts(app).is_empty()
        });
        assert_eq!(parts(&mut app).len(), expected);

        // A file that isn't there is asked for once, and says so in the log; another entity
        // pointed at the same model shares what is loaded.
        let ghost = app
            .world
            .spawn((Transform::IDENTITY, Model::new("ghost.glb")));
        frames(&mut app, &|app| {
            app.world.resource::<AssetServer>().loading() == 0
        });
        app.update();
        assert!(app.world.get::<ModelShown>(ghost).is_none());
        assert_eq!(
            app.world.resource::<AssetServer>().loading(),
            0,
            "not tried again"
        );
        let second = app
            .world
            .spawn((Transform::IDENTITY, Model::new("bird.glb")));
        app.update();
        assert!(app.world.get::<ModelShown>(second).is_some());
    }

    #[test]
    fn images_load_by_name_once_and_reload_when_saved() {
        let dir = TempDir::new("images");
        dir.png("red.png", [255, 0, 0, 255]);
        let mut server = AssetServer::new(&dir.0);
        let mut images = Assets::<Image>::default();

        let red = server.load_image(&mut images, "red.png");
        assert_eq!(
            server.load_image(&mut images, "red.png"),
            red,
            "one name, one handle"
        );
        assert_eq!(server.name_of(red), Some("red.png"));
        assert_eq!(server.find::<Image>("red.png"), Some(red));
        server.wait(&mut images);
        assert_eq!(pixel(&images, red), Some([255, 0, 0, 255]));
        assert!(images.get(red).unwrap().srgb);

        // The same file read as data is a different asset.
        let linear = server.load_image(&mut images, "red.png?linear");
        assert_ne!(linear, red);
        server.wait(&mut images);
        assert!(!images.get(linear).unwrap().srgb);

        // A file that isn't there: the handle exists, nothing arrives, nothing is stuck.
        let missing = server.load_image(&mut images, "nowhere.png");
        server.wait(&mut images);
        assert_eq!((pixel(&images, missing), server.loading()), (None, 0));

        // Saved again: reloaded under the same handle.
        assert_eq!(server.reload_changed(), 0);
        dir.png("red.png", [0, 0, 255, 255]);
        assert_eq!(server.reload_changed(), 2, "both readings of the file");
        server.wait(&mut images);
        assert_eq!(pixel(&images, red), Some([0, 0, 255, 255]));

        // A save that breaks the file leaves the old image.
        std::fs::write(dir.0.join("red.png"), b"not a picture").unwrap();
        server.reload_changed();
        server.wait(&mut images);
        assert_eq!(pixel(&images, red), Some([0, 0, 255, 255]));
    }

    #[test]
    fn the_frame_loop_waits_for_a_file_to_settle() {
        let dir = TempDir::new("settle");
        dir.png("a.png", [1, 2, 3, 255]);
        let mut server = AssetServer::new(&dir.0);
        server.hot_reload = true;
        server.check_interval = Duration::ZERO;
        let mut images = Assets::<Image>::default();
        let handle = server.load_image(&mut images, "a.png");
        server.wait(&mut images);

        dir.png("a.png", [9, 9, 9, 255]);
        server.check_files();
        assert_eq!(server.loading(), 0, "seen, not yet read");
        server.check_files();
        assert_eq!(server.loading(), 1);
        server.wait(&mut images);
        assert_eq!(pixel(&images, handle), Some([9, 9, 9, 255]));
    }

    #[test]
    fn shapes_have_names() {
        let mut server = AssetServer::default();
        let mut meshes = Assets::<Mesh>::default();
        let cube = server.cube(&mut meshes, 1.0);
        assert_eq!(server.name_of(cube), Some("shape:cube:1"));
        assert_eq!(server.cube(&mut meshes, 1.0), cube);
        assert_ne!(server.cube(&mut meshes, 2.0), cube);
        assert_eq!(server.shape(&mut meshes, "shape:cube:1").unwrap(), cube);
        assert_eq!(meshes.len(), 2);
        for bad in [
            "shape:blob:1",
            "shape:cube:-1",
            "shape:cube",
            "cube:1",
            "shape:cube:1:2",
        ] {
            assert!(server.shape(&mut meshes, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_model_file_is_loaded_once_and_its_parts_are_named() {
        let mut server = AssetServer::new(env!("CARGO_MANIFEST_DIR"));
        let (mut meshes, mut images) = (Assets::<Mesh>::default(), Assets::<Image>::default());
        let name = "res/paris/models/animals/hen_white.glb";
        let hen = server.load_gltf(name, &mut meshes, &mut images).unwrap();
        assert!(!hen.parts.is_empty());
        let first = hen.parts[0].mesh;
        assert_eq!(
            server.name_of(first),
            Some(format!("{name}#mesh0").as_str())
        );
        for part in &hen.parts {
            assert!(server.name_of(part.mesh).is_some());
        }
        let again = server.load_gltf(name, &mut meshes, &mut images).unwrap();
        assert!(Arc::ptr_eq(&hen, &again));
        assert!(server
            .load_gltf("res/nothing.glb", &mut meshes, &mut images)
            .is_err());
    }

    #[test]
    fn an_animated_model_plays_its_clip_and_is_saved_mid_stride() {
        use crate::{
            app::{App, Stage},
            ecs::Entity,
            render::{Animator, Playing},
            transform::{Parent, TransformPlugin},
        };
        // The first model in the demo that has both a skeleton and clips to play.
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("res/paris/models");
        let mut found = None;
        for name in ["citizen.glb", "gentleman.glb", "grenadier.glb", "boy1.glb"] {
            let scene = GltfScene::import(models.join(name))
                .and_then(|imported| {
                    GltfScene::build(imported, &mut Assets::default(), &mut Assets::default())
                })
                .unwrap();
            if scene.skeleton.is_some() && !scene.clips.is_empty() {
                found = Some((name, scene.clips.last().unwrap().name.clone()));
                break;
            }
        }
        let (file, clip) = found.expect("a model with a skeleton and clips");

        let build = || {
            let mut app = App::new();
            app.add_plugins(TransformPlugin)
                .insert_resource(AssetServer::new(&models))
                .init_resource::<Assets<Mesh>>()
                .init_resource::<Assets<Image>>()
                .register_type::<Model>()
                .register_type::<Playing>()
                .add_systems(Stage::PreUpdate, (update_asset_server, show_models))
                .add_systems(Stage::PostUpdate, crate::render::follow_playing);
            app
        };
        let arrive = |app: &mut App, figure: Entity| {
            for _ in 0..5000 {
                app.update();
                if app.world.get::<Animator>(figure).is_some() {
                    // Once more, for the animator to take up what is playing.
                    app.update();
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("the model did not arrive");
        };

        let mut app = build();
        let playing = Playing::new(clip.clone()).at(0.4);
        let figure = app.world.spawn((Transform::IDENTITY, Model::new(file), playing));
        arrive(&mut app, figure);
        // Its skinned parts are under it, each with a mesh of its own to be posed.
        let skinned = app
            .world
            .query::<(&Parent, &Skinned, &Mesh3d)>()
            .iter()
            .filter(|(parent, skinned, mesh)| parent.0 == figure && mesh.0 != skinned.source)
            .count();
        assert!(skinned > 0);
        let animator = app.world.get::<Animator>(figure).unwrap();
        assert_eq!(animator.clip(&clip), animator.clip(animator.playing()));
        assert!((animator.time() - 0.4).abs() < 1e-4);

        // The game moves it on; what is saved is the clip and the moment, not the skeleton.
        app.world.get_mut::<Animator>(figure).unwrap().seek(0.9);
        app.update();
        assert!((app.world.get::<Playing>(figure).unwrap().time - 0.9).abs() < 1e-4);
        let scene = app.world.resource_scope(|world, registry: &mut TypeRegistry| {
            Scene::capture(world, registry)
        });
        assert_eq!(scene.entities.len(), 1, "the parts are not saved");

        // In another run the figure comes back at that moment of that clip.
        let mut again = build();
        let spawned = again.world.resource_scope(|world, registry: &mut TypeRegistry| {
            scene.spawn(world, registry).entities
        });
        let figure = spawned[0];
        arrive(&mut again, figure);
        let animator = again.world.get::<Animator>(figure).unwrap();
        assert_eq!(animator.clip(&clip), animator.clip(animator.playing()));
        assert!((animator.time() - 0.9).abs() < 1e-4);

        // Asked for another clip, the animator turns to it.
        let other = animator.clips[0].name.clone();
        if animator.clip(&other) != animator.clip(&clip) {
            again.world.get_mut::<Playing>(figure).unwrap().clip = other.clone();
            again.update();
            let animator = again.world.get::<Animator>(figure).unwrap();
            assert_eq!(animator.clip(&other), animator.clip(animator.playing()));
        }
    }

    #[test]
    fn pictures_are_compressed_once_and_read_from_the_cache_after() {
        let dir = TempDir::new("compress");
        let save = |name: &str, side: u32, shade: u8| {
            image::RgbaImage::from_fn(side, side, |x, y| image::Rgba([(x * 30) as u8, (y * 30) as u8, shade, 255]))
                .save(dir.0.join(name))
                .unwrap();
        };
        save("wall.png", 8, 40);
        save("odd.png", 6, 40);
        let load = |keep_pixels: bool, name: &str| {
            let mut server = AssetServer::new(&dir.0);
            server.compress = true;
            server.keep_pixels = keep_pixels;
            let mut images = Assets::<Image>::default();
            let handle = server.load_image(&mut images, name);
            server.wait(&mut images);
            images.get(handle).cloned().unwrap()
        };
        let cached = || std::fs::read_dir(dir.0.join(".mira/cache")).map_or(0, |files| files.count());

        // Compressed, its pixels let go, and kept on disk under its bytes' hash.
        let wall = load(false, "wall.png");
        let processed = wall.processed.clone().expect("it was compressed");
        assert_eq!(processed.format, wgpu::TextureFormat::Bc7RgbaUnormSrgb);
        assert_eq!((wall.width, wall.height, wall.has_pixels()), (8, 8, false));
        assert_eq!(cached(), 1);
        // The same bytes again come from the cache, the same; with the pixels if wanted.
        let again = load(false, "wall.png");
        assert_eq!(again.processed.as_deref(), Some(&*processed));
        let kept = load(true, "wall.png");
        assert!(kept.has_pixels() && kept.processed.is_some());
        assert_eq!(cached(), 1);
        // As data and not colour it is another thing, and so is the file once it changes.
        let linear = load(false, "wall.png?linear");
        assert_eq!(linear.processed.unwrap().format, wgpu::TextureFormat::Bc7RgbaUnorm);
        save("wall.png", 8, 200);
        assert_ne!(load(false, "wall.png").processed.as_deref(), Some(&*processed));
        assert_eq!(cached(), 3);
        // A size that blocks can't hold stays as plain pixels.
        let odd = load(false, "odd.png");
        assert!(odd.processed.is_none() && odd.has_pixels());
        assert_eq!(cached(), 3);
    }

    #[test]
    fn a_model_files_textures_are_compressed_where_the_file_is_read() {
        let dir = TempDir::new("model-textures");
        let hen =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("res/paris/models/animals/hen_white.glb");
        std::fs::copy(&hen, dir.0.join("bird.glb")).unwrap();
        let load = |compress: bool| {
            let mut server = AssetServer::new(&dir.0);
            server.compress = compress;
            let (mut meshes, mut images) = (Assets::<Mesh>::default(), Assets::<Image>::default());
            assert!(!server.request_gltf("bird.glb"));
            server.wait_for_models(&mut meshes, &mut images);
            let scene = server.model("bird.glb").expect("the model arrived");
            let textures: Vec<Image> = scene
                .parts
                .iter()
                .filter_map(|part| part.material.base_color_texture)
                .filter_map(|handle| images.get(handle).cloned())
                .collect();
            assert!(!textures.is_empty(), "the hen has a texture");
            textures
        };
        // Sides that blocks can hold are compressed and keep no pixels; without compression
        // they are as they were.
        let fits = |image: &Image| image.width.is_multiple_of(4) && image.height.is_multiple_of(4);
        for texture in load(true) {
            assert_eq!(texture.processed.is_some(), fits(&texture));
            assert_eq!(texture.has_pixels(), !fits(&texture));
        }
        assert!(load(false).iter().all(|texture| texture.processed.is_none() && texture.has_pixels()));
    }

    #[test]
    fn assets_say_what_they_need_and_when_all_of_it_is_there() {
        let dir = TempDir::new("needs");
        dir.png("wall.png", [9, 9, 9, 255]);
        let mut server = AssetServer::new(&dir.0);
        let mut images = Assets::<Image>::default();
        // A level needs a house, the house a wall and a door, and the door the house.
        server.set_dependencies("level", ["house".to_owned()]);
        server.set_dependencies("house", ["wall.png".to_owned(), "door".to_owned()]);
        server.depends_on("door", "house");
        server.depends_on("door", "door");
        assert_eq!(server.dependencies("house"), ["door", "wall.png"]);
        assert_eq!(server.all_dependencies("level"), ["door", "house", "wall.png"]);
        assert_eq!(server.all_dependencies("door"), ["house", "wall.png"]);
        assert_eq!(server.dependents("house"), ["door", "level"]);
        assert!(server.dependencies("wall.png").is_empty());

        // Nothing has asked for the wall yet; then it is loading; then it is there.
        assert_eq!(server.state("wall.png"), AssetState::Unknown);
        assert!(!server.is_ready("level"));
        server.load_image(&mut images, "wall.png");
        assert_eq!(server.state("wall.png"), AssetState::Loading);
        assert!(!server.is_ready("level"));
        server.wait(&mut images);
        assert_eq!(server.state("wall.png"), AssetState::Loaded);
        assert!(server.is_ready("level"));

        // A picture that isn't there fails, and so does not hold up what doesn't need it.
        server.load_image(&mut images, "missing.png");
        server.wait(&mut images);
        assert_eq!(server.state("missing.png"), AssetState::Failed);
        server.depends_on("level", "missing.png");
        assert!(!server.is_ready("level") && server.is_ready("house"));

        // Said again, what an asset needs is what was said last.
        server.set_dependencies("level", Vec::new());
        assert!(server.dependencies("level").is_empty());
        assert!(server.known().contains(&"door".to_owned()));
    }

    #[test]
    fn a_model_file_and_its_parts_need_each_other() {
        let mut server = AssetServer::new(env!("CARGO_MANIFEST_DIR"));
        let (mut meshes, mut images) = (Assets::<Mesh>::default(), Assets::<Image>::default());
        let name = "res/paris/models/animals/hen_white.glb";
        assert_eq!(server.state(name), AssetState::Unknown);
        server.load_gltf(name, &mut meshes, &mut images).unwrap();
        let part = format!("{name}#mesh0");
        assert!(server.dependencies(name).contains(&part.as_str()));
        assert_eq!(server.dependencies(&part), [name]);
        assert!(server.is_ready(&part) && server.is_ready(name));
    }

    fn world(root: &Path) -> (World, TypeRegistry) {
        let mut world = World::new();
        world.insert_resource(AssetServer::new(root));
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<Image>>();
        let mut registry = TypeRegistry::default();
        registry.register::<Transform>();
        registry.register::<Mesh3d>();
        registry.register::<Material>();
        (world, registry)
    }

    fn wait(world: &mut World) {
        world.resource_scope(|world, server: &mut AssetServer| {
            server.wait(world.resource_mut::<Assets<Image>>());
        });
    }

    #[test]
    fn a_scene_with_assets_means_the_same_in_another_run() {
        let dir = TempDir::new("scene");
        dir.png("bricks.png", [200, 100, 50, 255]);
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("res/paris/models/animals/hen_white.glb"),
            dir.0.join("hen.glb"),
        )
        .unwrap();

        // The first run: a textured cube and a hen.
        let (mut first, registry) = world(&dir.0);
        let (cube, bricks, hen) = first.resource_scope(|world, server: &mut AssetServer| {
            world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                let images = world.resource_mut::<Assets<Image>>();
                let cube = server.cube(meshes, 1.0);
                let bricks = server.load_image(images, "bricks.png");
                let hen = server.load_gltf("hen.glb", meshes, images).unwrap();
                (cube, bricks, hen.parts[0].mesh)
            })
        });
        first.spawn((
            Transform::IDENTITY,
            Mesh3d(cube),
            Material {
                base_color_texture: Some(bricks),
                ..Default::default()
            },
        ));
        first.spawn((Transform::from_xyz(2.0, 0.0, 0.0), Mesh3d(hen)));
        let text = Scene::capture(&first, &registry).to_json();
        assert!(
            text.contains(r#"{"$asset": "mesh", "name": "shape:cube:1"}"#),
            "{text}"
        );
        assert!(
            text.contains(r#"{"$asset": "image", "name": "bricks.png"}"#),
            "{text}"
        );
        assert!(
            text.contains(r#"{"$asset": "mesh", "name": "hen.glb#mesh0"}"#),
            "{text}"
        );

        // A later run: nothing is loaded, and other assets already hold the low ids.
        let (mut second, registry) = world(&dir.0);
        for _ in 0..5 {
            second.resource_mut::<Assets<Mesh>>().add(Mesh::plane(1.0));
            second
                .resource_mut::<Assets<Image>>()
                .add(Image::solid([0; 4], true));
        }
        let spawned = Scene::from_json(&text)
            .unwrap()
            .spawn(&mut second, &registry);
        assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
        wait(&mut second);

        let server = second.resource::<AssetServer>();
        let new_cube = second.get::<Mesh3d>(spawned.entities[0]).unwrap().0;
        assert_ne!(new_cube, cube, "a different id this time");
        assert_eq!(server.name_of(new_cube), Some("shape:cube:1"));
        assert_eq!(
            second
                .resource::<Assets<Mesh>>()
                .get(new_cube)
                .unwrap()
                .vertices
                .len(),
            24
        );
        let texture = second
            .get::<Material>(spawned.entities[0])
            .unwrap()
            .base_color_texture
            .unwrap();
        assert_eq!(
            pixel(second.resource::<Assets<Image>>(), texture),
            Some([200, 100, 50, 255])
        );
        let new_hen = second.get::<Mesh3d>(spawned.entities[1]).unwrap().0;
        assert_eq!(server.name_of(new_hen), Some("hen.glb#mesh0"));
        assert_eq!(
            second
                .resource::<Assets<Mesh>>()
                .get(new_hen)
                .unwrap()
                .vertices
                .len(),
            first
                .resource::<Assets<Mesh>>()
                .get(hen)
                .unwrap()
                .vertices
                .len()
        );
    }

    #[test]
    fn assets_that_cannot_be_loaded_are_reported() {
        let registry = {
            let mut registry = TypeRegistry::default();
            registry.register::<Mesh3d>();
            registry
        };
        let text = r#"{"version": 1, "entities": [
            {"id": 1, "components": {"mira.Mesh3d": {"$asset": "mesh", "name": "shape:blob:1"}}},
            {"id": 2, "components": {"mira.Mesh3d": {"$asset": "sound", "name": "bang.wav"}}}
        ]}"#;
        let scene = Scene::from_json(text).unwrap();

        let (mut with_server, _) = world(Path::new("."));
        let spawned = scene.spawn(&mut with_server, &registry);
        assert_eq!(spawned.skipped.len(), 2, "{:?}", spawned.skipped);
        assert!(spawned.skipped[0].contains("there is no shape called `blob`"));
        assert!(spawned.skipped[1].contains("nothing knows how to load a `sound`"));

        // A world with no asset server at all.
        let spawned = scene.spawn(&mut World::new(), &registry);
        assert!(spawned.skipped[0].contains("this app has no asset server"));
    }
    #[test]
    fn unused_assets_are_unloaded_and_come_back_when_asked_for() {
        let dir = TempDir::new("unload");
        for name in ["wall.png", "floor.png", "sign.png"] {
            dir.png(name, [9, 9, 9, 255]);
        }
        std::fs::copy(
            "res/paris/models/animals/hen_white.glb",
            dir.0.join("hen.glb"),
        )
        .unwrap();
        std::fs::copy(
            "res/paris/models/animals/hen_white.glb",
            dir.0.join("cock.glb"),
        )
        .unwrap();
        let (mut world, registry) = world(&dir.0);
        world.insert_resource(registry);

        let (wall, floor, sign, cube, sphere, hen, cock) =
            world.resource_scope(|world, server: &mut AssetServer| {
                world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                    let images = world.resource_mut::<Assets<Image>>();
                    (
                        server.load_image(images, "wall.png"),
                        server.load_image(images, "floor.png"),
                        server.load_image(images, "sign.png"),
                        server.cube(meshes, 1.0),
                        server.sphere(meshes, 2.0),
                        server.load_gltf("hen.glb", meshes, images).unwrap(),
                        server.load_gltf("cock.glb", meshes, images).unwrap(),
                    )
                })
            });
        // In use: the cube and the wall on one entity, one part of the hen on another, and
        // the sign held where the engine can't see it. The floor is still being decoded.
        world.spawn((
            Transform::IDENTITY,
            Mesh3d(cube),
            Material {
                base_color_texture: Some(wall),
                ..Default::default()
            },
        ));
        world.spawn((Transform::IDENTITY, Mesh3d(hen.parts[0].mesh)));
        world.resource_mut::<AssetServer>().keep(sign);
        let hen_assets = |server: &AssetServer, file: &str| {
            server
                .names
                .values()
                .filter(|(_, name)| file_part(name) == file)
                .count()
        };
        let whole_hen = hen_assets(world.resource::<AssetServer>(), "hen.glb");
        let whole_cock = hen_assets(world.resource::<AssetServer>(), "cock.glb");
        assert!(whole_cock > 0);

        let unloaded = AssetServer::unload_unused(&mut world);
        assert_eq!(
            unloaded,
            2 + whole_cock,
            "the floor, the sphere and the whole cock"
        );
        wait(&mut world);
        let server = world.resource::<AssetServer>();
        assert_eq!(server.find::<Image>("wall.png"), Some(wall));
        assert_eq!(server.find::<Image>("sign.png"), Some(sign));
        assert_eq!(server.find::<Image>("floor.png"), None);
        assert_eq!(server.find::<Mesh>("shape:sphere:2"), None);
        assert_eq!(
            hen_assets(server, "hen.glb"),
            whole_hen,
            "a model in use stays whole"
        );
        assert_eq!(hen_assets(server, "cock.glb"), 0);
        let (meshes, images) = (
            world.resource::<Assets<Mesh>>(),
            world.resource::<Assets<Image>>(),
        );
        assert!(meshes.get(cube).is_some() && meshes.get(sphere).is_none());
        assert!(
            meshes.get(cock.parts[0].mesh).is_none() && meshes.get(hen.parts[0].mesh).is_some()
        );
        assert!(images.get(wall).is_some() && images.get(sign).is_some());
        assert!(
            images.get(floor).is_none(),
            "its decode arrived to find it unwanted"
        );
        assert_eq!(
            AssetServer::unload_unused(&mut world),
            0,
            "nothing more to unload"
        );

        // Asked for again, they load again, under new handles.
        let (floor_again, cock_again) = world.resource_scope(|world, server: &mut AssetServer| {
            world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                let images = world.resource_mut::<Assets<Image>>();
                (
                    server.load_image(images, "floor.png"),
                    server.load_gltf("cock.glb", meshes, images).unwrap(),
                )
            })
        });
        wait(&mut world);
        assert_ne!(floor_again, floor);
        assert_eq!(
            pixel(world.resource::<Assets<Image>>(), floor_again),
            Some([9, 9, 9, 255])
        );
        assert!(world
            .resource::<Assets<Mesh>>()
            .get(cock_again.parts[0].mesh)
            .is_some());

        // Released and no longer on any entity, the rest go too.
        world.resource_mut::<AssetServer>().release(sign);
        let entities: Vec<_> = world.query::<crate::ecs::Entity>().iter().collect();
        for entity in entities {
            world.despawn(entity);
        }
        assert!(AssetServer::unload_unused(&mut world) > whole_hen);
        assert!(world.resource::<AssetServer>().names.is_empty());
        assert!(world.resource::<Assets<Mesh>>().is_empty());
        wait(&mut world);
        assert!(world.resource::<Assets<Image>>().is_empty());
    }
}
