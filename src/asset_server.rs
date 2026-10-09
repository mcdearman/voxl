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
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;

use crate::{
    assets::{Assets, Handle},
    ecs::{ResMut, World},
    reflect::{TypeRegistry, Value},
    render::{GltfScene, Image, Mesh, Skinned},
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

struct DecodedImage {
    id: u32,
    path: PathBuf,
    image: anyhow::Result<Image>,
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
    last_check: Option<Instant>,
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
            last_check: None,
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
            let in_use = |key: &(TypeId, u32), kind: &str, name: &str| {
                used.contains(key) || named.contains(&(kind.to_owned(), name.to_owned()))
            };
            // A model file stays or goes whole: its parts refer to each other.
            let files: HashSet<String> = server
                .names
                .iter()
                .filter(|(key, (kind, name))| name.contains('#') && in_use(key, kind, name))
                .map(|(_, (_, name))| file_part(name).to_owned())
                .collect();
            let unused: Vec<(TypeId, u32, String)> = server
                .names
                .iter()
                .filter(|(key, (kind, name))| {
                    !in_use(key, kind, name)
                        && !(name.contains('#') && files.contains(file_part(name)))
                })
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
                server.ids.remove(&(asset_type, name));
                unloaded += 1;
            }
            server.gltf.retain(|file, _| files.contains(file));
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
        let pool = self.pool.get_or_insert_with(|| TaskPool::new(2));
        pool.spawn(&self.decoded.sender, move || {
            let image = std::fs::read(&path)
                .with_context(|| format!("can't read {}", path.display()))
                .and_then(|bytes| {
                    Image::from_bytes(&bytes, srgb)
                        .with_context(|| format!("can't decode {}", path.display()))
                });
            DecodedImage { id, path, image }
        });
    }

    /// How many assets are still being loaded.
    pub fn loading(&self) -> usize {
        self.loading
    }

    fn accept(&mut self, images: &mut Assets<Image>, decoded: DecodedImage) {
        self.loading -= 1;
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
            Err(err) => log::error!("{err:#}"),
        }
    }

    /// Takes in the images that have finished decoding. Returns how many arrived.
    pub fn finish(&mut self, images: &mut Assets<Image>) -> usize {
        let mut arrived = 0;
        while let Ok(decoded) = self.decoded.receiver.try_recv() {
            self.accept(images, decoded);
            arrived += 1;
        }
        arrived
    }

    /// Waits until nothing is still loading. For tools and tests; a game should carry on and
    /// let assets arrive.
    pub fn wait(&mut self, images: &mut Assets<Image>) {
        while self.loading > 0 {
            match self.decoded.receiver.recv() {
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
        changed.len()
    }

    fn reload(&mut self, index: usize) {
        let watched = &mut self.watched[index];
        watched.current = stamp(&watched.path);
        watched.settling = None;
        let (id, path, srgb) = (watched.id, watched.path.clone(), watched.srgb);
        self.decode(id, path, srgb);
    }

    fn check_files(&mut self) {
        if !self.hot_reload || self.watched.is_empty() {
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
        let (first_mesh, first_image) = (meshes.next_id(), images.next_id());
        let scene = Arc::new(GltfScene::load(self.path(name), meshes, images)?);
        // Whatever the loader added is this file's, in an order that depends only on the file.
        for id in first_mesh..meshes.next_id() {
            self.name(
                "mesh",
                Handle::<Mesh>::from_id(id),
                format!("{name}#mesh{}", id - first_mesh),
            );
        }
        for id in first_image..images.next_id() {
            self.name(
                "image",
                Handle::<Image>::from_id(id),
                format!("{name}#image{}", id - first_image),
            );
        }
        self.gltf.insert(name.to_owned(), scene.clone());
        Ok(scene)
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
) {
    if server.loading > 0 {
        server.finish(&mut images);
    }
    server.check_files();
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
