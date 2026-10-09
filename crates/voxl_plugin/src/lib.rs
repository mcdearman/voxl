//! Bindings for writing voxl native plugins in Rust.
//!
//! A plugin is a `cdylib` crate that depends on this one (and not on the engine, so it builds
//! in a second or two) and calls [`export_plugin!`]:
//!
//! ```ignore
//! use voxl_plugin::{App, Component, Error, Stage, System, Transform};
//!
//! static mut TRANSFORM: Option<Component<Transform>> = None;
//!
//! fn load(app: &mut App) -> Result<(), Error> {
//!     let transform = app.lookup::<Transform>("voxl.Transform")?;
//!     app.add_system("rise", Stage::Update, &[transform.write()], rise)
//! }
//!
//! fn rise(system: &mut System) {
//!     let dt = system.delta();
//!     while let Some((_entity, [transform])) = system.next() {
//!         let transform = unsafe { &mut *transform.cast::<Transform>() };
//!         transform.translation[1] += dt;
//!     }
//! }
//!
//! voxl_plugin::export_plugin!(load);
//! ```
//!
//! `load` runs when the plugin is first loaded and again after every hot reload. Component
//! values and [`App::state`] blocks survive a reload; the plugin's own statics do not.
//!
//! The interface itself is C (see `include/voxl.h` and [`sys`]); this crate is a thin layer
//! over it, and plugins in other languages use the header directly.

pub mod sys;

use std::{
    ffi::c_void,
    fmt,
    marker::PhantomData,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicPtr, Ordering},
};

pub use sys::{
    VoxlBody as Body, VoxlCamera as Camera, VoxlCollider as Collider, VoxlContact as Contact,
    VoxlLight as Light, VoxlMaterial as Material, VoxlQuat as Quat, VoxlRayHit as RayHit,
    VoxlTransform as Transform, VoxlVertex as Vertex,
};

static API: AtomicPtr<sys::VoxlApi> = AtomicPtr::new(std::ptr::null_mut());

fn api() -> &'static sys::VoxlApi {
    let api = API.load(Ordering::Relaxed);
    assert!(
        !api.is_null(),
        "the voxl API is only available while the plugin is loaded"
    );
    // SAFETY: the engine keeps the table alive until `voxl_plugin_unload` returns.
    unsafe { &*api }
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// An entity handle. Valid until the entity is despawned.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Entity(pub u64);

/// When in the frame a system runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Once, the first time the plugin is loaded (not again on reload).
    Startup = sys::VOXL_STAGE_STARTUP as isize,
    First = sys::VOXL_STAGE_FIRST as isize,
    PreUpdate = sys::VOXL_STAGE_PRE_UPDATE as isize,
    /// Fixed timestep: zero or more times per frame.
    FixedUpdate = sys::VOXL_STAGE_FIXED_UPDATE as isize,
    Update = sys::VOXL_STAGE_UPDATE as isize,
    PostUpdate = sys::VOXL_STAGE_POST_UPDATE as isize,
    Last = sys::VOXL_STAGE_LAST as isize,
}

/// A component whose values are `T`.
pub struct Component<T> {
    id: sys::VoxlComponent,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Component<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Component<T> {}

impl<T> Component<T> {
    pub fn id(self) -> sys::VoxlComponent {
        self.id
    }

    fn term(self, access: u32) -> Term {
        Term(sys::VoxlTerm {
            component: self.id,
            access,
        })
    }

    /// Visit entities that have this component, and get a pointer to read it.
    pub fn read(self) -> Term {
        self.term(sys::VOXL_READ)
    }

    /// Visit entities that have this component, and get a pointer to change it.
    pub fn write(self) -> Term {
        self.term(sys::VOXL_WRITE)
    }

    /// Only visit entities that have this component.
    pub fn with(self) -> Term {
        self.term(sys::VOXL_WITH)
    }

    /// Only visit entities that don't have this component.
    pub fn without(self) -> Term {
        self.term(sys::VOXL_WITHOUT)
    }
}

/// One part of a system's query. Made by the methods on [`Component`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Term(sys::VoxlTerm);

/// What one field of a component holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    F32 = sys::VOXL_FIELD_F32 as isize,
    F64 = sys::VOXL_FIELD_F64 as isize,
    I32 = sys::VOXL_FIELD_I32 as isize,
    I64 = sys::VOXL_FIELD_I64 as isize,
    U8 = sys::VOXL_FIELD_U8 as isize,
    U32 = sys::VOXL_FIELD_U32 as isize,
    /// One byte; zero is false.
    Bool = sys::VOXL_FIELD_BOOL as isize,
    /// An [`Entity`]. Scenes keep it pointing at the right entity.
    Entity = sys::VOXL_FIELD_ENTITY as isize,
}

/// One field of a component, for [`App::describe`]: `count` values of one type (more than one
/// for a vector or an array) starting `offset` bytes in. Use `std::mem::offset_of!` for the
/// offset.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub field_type: FieldType,
    pub count: u32,
    pub offset: usize,
}

impl Field {
    pub fn new(name: &'static str, field_type: FieldType, offset: usize) -> Self {
        Self {
            name,
            field_type,
            count: 1,
            offset,
        }
    }

    /// `count` values in a row: a vector or an array.
    pub fn array(name: &'static str, field_type: FieldType, count: u32, offset: usize) -> Self {
        Self {
            name,
            field_type,
            count,
            offset,
        }
    }
}

/// An image, from [`System::load_image`]. Just a number, so it can be kept in [`App::state`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image(pub u32);

/// An event type whose events are `T`, from [`App::event`].
pub struct Event<T> {
    id: sys::VoxlEvent,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Event<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Event<T> {}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y: 60f32.to_radians(),
            near: 0.1,
            active: 1,
        }
    }
}

impl Default for Light {
    fn default() -> Self {
        Self {
            color: [1.0; 3],
            intensity: 3.0,
            shadows: 1,
        }
    }
}

impl Collider {
    fn of(shape: u32, size: [f32; 3]) -> Self {
        Self {
            shape,
            size,
            friction: 0.5,
            restitution: 0.0,
            sensor: 0,
        }
    }

    pub fn sphere(radius: f32) -> Self {
        Self::of(sys::VOXL_COLLIDER_SPHERE, [radius, 0.0, 0.0])
    }

    /// A box with the given half extents.
    pub fn cuboid(half: [f32; 3]) -> Self {
        Self::of(sys::VOXL_COLLIDER_BOX, half)
    }

    /// An upright capsule.
    pub fn capsule(height: f32, radius: f32) -> Self {
        Self::of(sys::VOXL_COLLIDER_CAPSULE, [radius, height, 0.0])
    }

    /// Everything below the entity's position.
    pub fn ground() -> Self {
        Self::of(sys::VOXL_COLLIDER_GROUND, [0.0; 3])
    }
}

impl Body {
    fn of(kind: u32) -> Self {
        Self {
            kind,
            mass: 0.0,
            velocity: [0.0; 3],
            lock_rotation: 0,
        }
    }

    /// Moved by gravity, forces and collisions.
    pub fn dynamic() -> Self {
        Self::of(sys::VOXL_BODY_DYNAMIC)
    }

    /// Moved only by its velocity; pushes other bodies and is never pushed.
    pub fn kinematic() -> Self {
        Self::of(sys::VOXL_BODY_KINEMATIC)
    }

    /// Moved by setting its transform; pushes other bodies and is never pushed.
    pub fn animated() -> Self {
        Self::of(sys::VOXL_BODY_ANIMATED)
    }
}

/// A further query of a system, from [`App::add_query`].
#[derive(Clone, Copy, Debug)]
pub struct Query {
    index: u32,
    pointers: usize,
}

/// A key, by its position on a US keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key(pub u32);

impl Key {
    pub const SPACE: Key = Key(sys::VOXL_KEY_SPACE);
    pub const ENTER: Key = Key(sys::VOXL_KEY_ENTER);
    pub const ESCAPE: Key = Key(sys::VOXL_KEY_ESCAPE);
    pub const TAB: Key = Key(sys::VOXL_KEY_TAB);
    pub const BACKSPACE: Key = Key(sys::VOXL_KEY_BACKSPACE);
    pub const LEFT: Key = Key(sys::VOXL_KEY_LEFT);
    pub const RIGHT: Key = Key(sys::VOXL_KEY_RIGHT);
    pub const UP: Key = Key(sys::VOXL_KEY_UP);
    pub const DOWN: Key = Key(sys::VOXL_KEY_DOWN);
    pub const LEFT_SHIFT: Key = Key(sys::VOXL_KEY_LEFT_SHIFT);
    pub const RIGHT_SHIFT: Key = Key(sys::VOXL_KEY_RIGHT_SHIFT);
    pub const LEFT_CONTROL: Key = Key(sys::VOXL_KEY_LEFT_CONTROL);
    pub const RIGHT_CONTROL: Key = Key(sys::VOXL_KEY_RIGHT_CONTROL);
    pub const LEFT_ALT: Key = Key(sys::VOXL_KEY_LEFT_ALT);
    pub const RIGHT_ALT: Key = Key(sys::VOXL_KEY_RIGHT_ALT);

    /// A letter key, `'a'..='z'` in either case.
    pub fn letter(letter: char) -> Option<Key> {
        let letter = letter.to_ascii_lowercase();
        letter
            .is_ascii_lowercase()
            .then(|| Key(sys::VOXL_KEY_A + (letter as u32 - 'a' as u32)))
    }

    /// A key on the digit row, 0 to 9.
    pub fn digit(digit: u32) -> Option<Key> {
        (digit <= 9).then(|| Key(sys::VOXL_KEY_0 + digit))
    }

    /// F1 to F12.
    pub fn function(number: u32) -> Option<Key> {
        (1..=12)
            .contains(&number)
            .then(|| Key(sys::VOXL_KEY_F1 + number - 1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left = sys::VOXL_MOUSE_LEFT as isize,
    Right = sys::VOXL_MOUSE_RIGHT as isize,
    Middle = sys::VOXL_MOUSE_MIDDLE as isize,
}

/// A mesh, shared between the entities drawn with it. Create it once and keep the handle in
/// [`App::state`]; it is just a number, so it survives a reload there.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mesh(pub u32);

impl Default for Material {
    fn default() -> Self {
        Self {
            color: [1.0; 4],
            emissive: [0.0; 3],
            roughness: 0.8,
            metallic: 0.0,
        }
    }
}

/// Types that are valid as any bit pattern, including all zeros, so the engine can hand out
/// zeroed memory for them.
///
/// # Safety
/// Every bit pattern of the right size must be a valid value: no references, no `bool`, no
/// enums, no padding-sensitive invariants.
pub unsafe trait Plain: Copy + 'static {}

macro_rules! impl_plain {
    ($($t:ty),*) => { $(unsafe impl Plain for $t {})* };
}
impl_plain!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);
unsafe impl Plain for Mesh {}
unsafe impl Plain for Image {}
unsafe impl<T: Plain, const N: usize> Plain for [T; N] {}

/// The app being set up. Only exists inside the plugin's `load` function.
pub struct App {
    raw: *mut sys::VoxlApp,
}

struct Callback {
    run: fn(&mut System),
    pointers: usize,
}

impl App {
    /// Defines a component stored as the bytes of `T`, or finds it if this plugin defined it
    /// before a reload (its values are kept as long as `T`'s size and alignment are unchanged).
    pub fn register<T: Copy + 'static>(&mut self, name: &str) -> Result<Component<T>, Error> {
        // SAFETY: called inside `load` with the engine's app pointer.
        let id = unsafe {
            (api().component_register)(
                self.raw,
                name.as_ptr(),
                name.len(),
                size_of::<T>(),
                align_of::<T>(),
                None,
            )
        };
        if id == 0 {
            return Err(format!("could not register component `{name}`").into());
        }
        Ok(Component {
            id,
            _marker: PhantomData,
        })
    }

    /// Finds a component defined by the engine or another plugin. Fails if it doesn't exist
    /// or `T` isn't the size and alignment the engine reports for it.
    pub fn lookup<T: 'static>(&mut self, name: &str) -> Result<Component<T>, Error> {
        let (mut size, mut align) = (0, 0);
        // SAFETY: as above; the out pointers are valid.
        let id = unsafe {
            (api().component_lookup)(self.raw, name.as_ptr(), name.len(), &mut size, &mut align)
        };
        if id == 0 {
            return Err(format!("no component named `{name}`").into());
        }
        if (size, align) != (size_of::<T>(), align_of::<T>()) {
            return Err(format!(
                "component `{name}` is {size} bytes aligned to {align}, but the plugin's type is \
                 {} bytes aligned to {}",
                size_of::<T>(),
                align_of::<T>()
            )
            .into());
        }
        Ok(Component {
            id,
            _marker: PhantomData,
        })
    }

    /// A zero-initialized `T` owned by the engine that survives reloads. The pointer stays
    /// valid until the plugin is unloaded; fetch it again in each `load`.
    pub fn state<T: Plain>(&mut self, name: &str) -> *mut T {
        // SAFETY: as above.
        let block = unsafe {
            (api().state)(
                self.raw,
                name.as_ptr(),
                name.len(),
                size_of::<T>(),
                align_of::<T>(),
            )
        };
        block.cast()
    }

    /// Says what fields a component this plugin registered has, so the engine can save it in
    /// scenes, load it back and show it in an inspector. Bytes no field covers aren't saved.
    pub fn describe<T>(&mut self, component: Component<T>, fields: &[Field]) -> Result<(), Error> {
        let raw: Vec<sys::VoxlField> = fields
            .iter()
            .map(|field| sys::VoxlField {
                name: field.name.as_ptr(),
                name_len: field.name.len(),
                field_type: field.field_type as u32,
                count: field.count,
                offset: field.offset,
            })
            .collect();
        // SAFETY: called inside `load`; `raw` and the names it points to outlive the call.
        let status =
            unsafe { (api().component_describe)(self.raw, component.id, raw.as_ptr(), raw.len()) };
        if status != 0 {
            return Err("could not describe the component (see the engine's log)".into());
        }
        Ok(())
    }

    /// Defines an event type whose events are the bytes of `T`, or finds the one this name
    /// already has. Any plugin that knows the name can send and read it; the engine's own
    /// events are found the same way (`"voxl.Contact"` is a [`Contact`]).
    pub fn event<T: Copy + 'static>(&mut self, name: &str) -> Result<Event<T>, Error> {
        // SAFETY: called inside `load` with the engine's app pointer.
        let id =
            unsafe { (api().event_register)(self.raw, name.as_ptr(), name.len(), size_of::<T>()) };
        if id == 0 {
            return Err(format!("could not register event `{name}` (is it another size?)").into());
        }
        Ok(Event {
            id,
            _marker: PhantomData,
        })
    }

    /// Gives the system called `system` (added earlier in this `load`) another query. Two
    /// queries of one system may only reach the same component if neither writes it, or if
    /// `with`/`without` terms guarantee they never match the same entity.
    pub fn add_query(&mut self, system: &str, terms: &[Term]) -> Result<Query, Error> {
        // SAFETY: called inside `load`; `Term` is a transparent wrapper over `VoxlTerm`.
        let index = unsafe {
            (api().system_add_query)(
                self.raw,
                system.as_ptr(),
                system.len(),
                terms.as_ptr().cast(),
                terms.len(),
            )
        };
        if index < 0 {
            return Err(format!("could not add a query to system `{system}`").into());
        }
        Ok(Query {
            index: index as u32,
            pointers: pointer_count(terms),
        })
    }

    /// Adds a system that visits every entity matching `terms`.
    pub fn add_system(
        &mut self,
        name: &str,
        stage: Stage,
        terms: &[Term],
        run: fn(&mut System),
    ) -> Result<(), Error> {
        let pointers = pointer_count(terms);
        // Leaked on purpose: the engine may call the system until the library is unloaded,
        // and a few bytes per system per reload is not worth tracking.
        let callback = Box::into_raw(Box::new(Callback { run, pointers }));
        let desc = sys::VoxlSystemDesc {
            name: name.as_ptr(),
            name_len: name.len(),
            stage: stage as u32,
            reserved: 0,
            run: Some(trampoline),
            user: callback.cast(),
            terms: terms.as_ptr().cast(),
            term_count: terms.len(),
        };
        // SAFETY: `desc` and everything it points to live for the call, which is all the
        // engine needs; `Term` is a transparent wrapper over `VoxlTerm`.
        if unsafe { (api().system_add)(self.raw, &desc) } != 0 {
            return Err(format!("could not add system `{name}`").into());
        }
        Ok(())
    }
}

fn pointer_count(terms: &[Term]) -> usize {
    terms
        .iter()
        .filter(|t| matches!(t.0.access, sys::VOXL_READ | sys::VOXL_WRITE))
        .count()
}

unsafe extern "C" fn trampoline(system: *mut sys::VoxlSystem, user: *mut c_void) {
    // SAFETY: `user` is the `Callback` leaked in `add_system`.
    let callback = &*user.cast::<Callback>();
    let mut system = System {
        raw: system,
        pointers: callback.pointers,
    };
    // A panic must not unwind into the engine.
    if catch_unwind(AssertUnwindSafe(|| (callback.run)(&mut system))).is_err() {
        log(sys::VOXL_LOG_ERROR, "a plugin system panicked");
    }
}

/// One run of a system. Only exists inside the system's function.
pub struct System {
    raw: *mut sys::VoxlSystem,
    pointers: usize,
}

impl System {
    /// Seconds since the last frame (or the fixed timestep, in `FixedUpdate`).
    pub fn delta(&self) -> f32 {
        // SAFETY: called inside the system with the engine's pointer.
        unsafe { (api().delta_seconds)(self.raw) }
    }

    /// Seconds since the app started.
    pub fn elapsed(&self) -> f64 {
        // SAFETY: as above.
        unsafe { (api().elapsed_seconds)(self.raw) }
    }

    fn check<const N: usize>(&self) {
        assert!(
            N == self.pointers,
            "this system has {} read/write terms, but {N} pointers were asked for",
            self.pointers
        );
    }

    /// The next matching entity, with one pointer per `read`/`write` term in the order the
    /// terms were given. The pointers are valid until the system returns; cast each to the
    /// component's type, and only write through those from `write` terms.
    #[allow(clippy::should_implement_trait)]
    pub fn next<const N: usize>(&mut self) -> Option<(Entity, [*mut c_void; N])> {
        self.check::<N>();
        let mut entity = sys::VOXL_ENTITY_NONE;
        let mut pointers = [std::ptr::null_mut(); N];
        // SAFETY: `pointers` has room for every read/write term (checked above).
        let found = unsafe { (api().query_next)(self.raw, &mut entity, pointers.as_mut_ptr()) };
        (found != 0).then_some((Entity(entity), pointers))
    }

    /// Looks one entity up directly. `None` if it doesn't match the system's terms.
    pub fn get<const N: usize>(&mut self, entity: Entity) -> Option<[*mut c_void; N]> {
        self.check::<N>();
        let mut pointers = [std::ptr::null_mut(); N];
        // SAFETY: as in `next`.
        let found = unsafe { (api().query_get)(self.raw, entity.0, pointers.as_mut_ptr()) };
        (found != 0).then_some(pointers)
    }

    /// As [`System::next`], for a further query of this system. Each query keeps its own place.
    pub fn next_in<const N: usize>(&mut self, query: Query) -> Option<(Entity, [*mut c_void; N])> {
        assert!(
            N == query.pointers,
            "this query has {} read/write terms",
            query.pointers
        );
        let mut entity = sys::VOXL_ENTITY_NONE;
        let mut pointers = [std::ptr::null_mut(); N];
        // SAFETY: `pointers` has room for every read/write term (checked above).
        let found = unsafe {
            (api().query_next_in)(self.raw, query.index, &mut entity, pointers.as_mut_ptr())
        };
        (found != 0).then_some((Entity(entity), pointers))
    }

    /// As [`System::get`], for a further query of this system.
    pub fn get_in<const N: usize>(
        &mut self,
        query: Query,
        entity: Entity,
    ) -> Option<[*mut c_void; N]> {
        assert!(
            N == query.pointers,
            "this query has {} read/write terms",
            query.pointers
        );
        let mut pointers = [std::ptr::null_mut(); N];
        // SAFETY: as in `next_in`.
        let found =
            unsafe { (api().query_get_in)(self.raw, query.index, entity.0, pointers.as_mut_ptr()) };
        (found != 0).then_some(pointers)
    }

    /// Starts a query again from its first entity. `None` rewinds the system's own query.
    pub fn rewind(&mut self, query: Option<Query>) {
        // SAFETY: called inside the system.
        unsafe { (api().query_rewind)(self.raw, query.map_or(0, |q| q.index)) }
    }

    /// Whether the key is held down.
    pub fn key_down(&self, key: Key) -> bool {
        // SAFETY: called inside the system.
        unsafe { (api().key_down)(self.raw, key.0) != 0 }
    }

    /// Whether the key went down this frame.
    pub fn key_pressed(&self, key: Key) -> bool {
        // SAFETY: as above.
        unsafe { (api().key_pressed)(self.raw, key.0) != 0 }
    }

    /// Whether the key came up this frame.
    pub fn key_released(&self, key: Key) -> bool {
        // SAFETY: as above.
        unsafe { (api().key_released)(self.raw, key.0) != 0 }
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        // SAFETY: as above.
        unsafe { (api().mouse_down)(self.raw, button as u32) != 0 }
    }

    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        // SAFETY: as above.
        unsafe { (api().mouse_pressed)(self.raw, button as u32) != 0 }
    }

    /// How far the mouse moved this frame, in pixels: x, then y.
    pub fn mouse_motion(&self) -> [f32; 2] {
        let mut delta = [0.0; 2];
        // SAFETY: `delta` has room for the two floats the engine writes.
        unsafe { (api().mouse_motion)(self.raw, delta.as_mut_ptr()) };
        delta
    }

    fn shape(&mut self, shape: u32, a: f32) -> Option<Mesh> {
        // SAFETY: called inside the system.
        let mesh = unsafe { (api().mesh_shape)(self.raw, shape, a) };
        (mesh != 0).then_some(Mesh(mesh))
    }

    /// A cube with the given edge length. `None` if the app has no renderer.
    pub fn mesh_cube(&mut self, size: f32) -> Option<Mesh> {
        self.shape(sys::VOXL_SHAPE_CUBE, size)
    }

    pub fn mesh_sphere(&mut self, radius: f32) -> Option<Mesh> {
        self.shape(sys::VOXL_SHAPE_SPHERE, radius)
    }

    /// A flat square facing up, with the given edge length.
    pub fn mesh_plane(&mut self, size: f32) -> Option<Mesh> {
        self.shape(sys::VOXL_SHAPE_PLANE, size)
    }

    /// A mesh from triangles: three indices each, counter-clockwise seen from the front.
    pub fn mesh_create(&mut self, vertices: &[Vertex], indices: &[u32]) -> Option<Mesh> {
        // SAFETY: the slices are valid for the lengths given, and the engine copies them.
        let mesh = unsafe {
            (api().mesh_create)(
                self.raw,
                vertices.as_ptr(),
                vertices.len(),
                indices.as_ptr(),
                indices.len(),
            )
        };
        (mesh != 0).then_some(Mesh(mesh))
    }

    /// Draws the entity as `mesh` (it also needs a `Transform`), when this system returns.
    pub fn set_mesh(&mut self, entity: Entity, mesh: Mesh) {
        // SAFETY: called inside the system.
        unsafe { (api().set_mesh)(self.raw, entity.0, mesh.0) }
    }

    /// Sets the entity's surface, when this system returns.
    pub fn set_material(&mut self, entity: Entity, material: &Material) {
        // SAFETY: `material` is valid for the call, and the engine copies it.
        unsafe { (api().set_material)(self.raw, entity.0, material) }
    }

    /// Sends an event when this system returns. Systems later this frame, and every system
    /// next frame, can read it.
    pub fn send<T: Copy + 'static>(&mut self, event: Event<T>, value: &T) {
        // SAFETY: `value` is `size_of::<T>()` readable bytes, the size the event was
        // registered with, and the engine copies them.
        unsafe { (api().event_send)(self.raw, event.id, std::ptr::from_ref(value).cast()) }
    }

    /// The next event this system hasn't read yet. Each system reads each event once, and a
    /// reloaded system carries on where the old one stopped.
    pub fn next_event<T: Copy + 'static>(&mut self, event: Event<T>) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        // SAFETY: the engine writes exactly the event's size, which is `size_of::<T>()`.
        unsafe {
            let found = (api().event_next)(self.raw, event.id, value.as_mut_ptr().cast());
            (found != 0).then(|| value.assume_init())
        }
    }

    /// Makes the entity a camera (it also needs a `Transform`), when this system returns.
    pub fn set_camera(&mut self, entity: Entity, camera: &Camera) {
        // SAFETY: `camera` is valid for the call, and the engine copies it.
        unsafe { (api().set_camera)(self.raw, entity.0, camera) }
    }

    /// Makes the entity a sun-like light shining along its transform's forward direction.
    pub fn set_light(&mut self, entity: Entity, light: &Light) {
        // SAFETY: as above.
        unsafe { (api().set_light)(self.raw, entity.0, light) }
    }

    /// Sets the light arriving from every direction.
    pub fn set_ambient(&mut self, color: [f32; 3], intensity: f32) {
        // SAFETY: `color` is three floats.
        unsafe { (api().set_ambient)(self.raw, color.as_ptr(), intensity) }
    }

    pub fn set_window_title(&mut self, title: &str) {
        // SAFETY: the engine copies the text before returning.
        unsafe { (api().set_window_title)(self.raw, title.as_ptr(), title.len()) }
    }

    /// Makes the entity solid. Does nothing in an app without physics.
    pub fn set_collider(&mut self, entity: Entity, collider: &Collider) {
        // SAFETY: `collider` is valid for the call, and the engine copies it.
        unsafe { (api().set_collider)(self.raw, entity.0, collider) }
    }

    /// Makes an entity with a collider move.
    pub fn set_body(&mut self, entity: Entity, body: &Body) {
        // SAFETY: as above.
        unsafe { (api().set_body)(self.raw, entity.0, body) }
    }

    /// A sudden push on a dynamic body, in newton-seconds.
    pub fn apply_impulse(&mut self, entity: Entity, impulse: [f32; 3]) {
        // SAFETY: `impulse` is three floats.
        unsafe { (api().apply_impulse)(self.raw, entity.0, impulse.as_ptr()) }
    }

    pub fn set_velocity(&mut self, entity: Entity, velocity: [f32; 3]) {
        // SAFETY: `velocity` is three floats.
        unsafe { (api().set_velocity)(self.raw, entity.0, velocity.as_ptr()) }
    }

    /// The body's velocity, or `None` if the entity has no body.
    pub fn velocity(&self, entity: Entity) -> Option<[f32; 3]> {
        let mut velocity = [0.0; 3];
        // SAFETY: `velocity` has room for the three floats the engine writes.
        let found = unsafe { (api().velocity)(self.raw, entity.0, velocity.as_mut_ptr()) };
        (found != 0).then_some(velocity)
    }

    /// The first collider a ray meets within `max_distance`, as of the last physics step.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
    ) -> Option<RayHit> {
        let mut hit = std::mem::MaybeUninit::<RayHit>::uninit();
        // SAFETY: the engine writes `hit` whenever it returns nonzero.
        unsafe {
            let found = (api().raycast)(
                self.raw,
                origin.as_ptr(),
                direction.as_ptr(),
                max_distance,
                hit.as_mut_ptr(),
            );
            (found != 0).then(|| hit.assume_init())
        }
    }

    /// Loads an image by name: a PNG or JPEG path relative to the app's asset folder, with
    /// `?linear` appended for data such as normal maps. The same name gives the same image.
    /// It arrives a moment later; until then it draws as plain white.
    pub fn load_image(&mut self, name: &str) -> Option<Image> {
        // SAFETY: called inside the system; the engine copies the name.
        let image = unsafe { (api().image_load)(self.raw, name.as_ptr(), name.len()) };
        (image != 0).then_some(Image(image))
    }

    /// Sets the textures of the entity's material, when this system returns.
    pub fn set_textures(
        &mut self,
        entity: Entity,
        base_color: Option<Image>,
        normal: Option<Image>,
        metallic_roughness: Option<Image>,
    ) {
        let raw = |image: Option<Image>| image.map_or(0, |image| image.0);
        // SAFETY: called inside the system.
        unsafe {
            (api().set_textures)(
                self.raw,
                entity.0,
                raw(base_color),
                raw(normal),
                raw(metallic_roughness),
            )
        }
    }

    /// Spawns a glTF model by name: a new entity at `transform` with a child for each part of
    /// the model. The parts appear when this system returns.
    pub fn spawn_model(&mut self, name: &str, transform: &Transform) -> Entity {
        // SAFETY: called inside the system; the engine copies the name and the transform.
        Entity(unsafe { (api().spawn_model)(self.raw, name.as_ptr(), name.len(), transform) })
    }

    /// Spawns an instance of a prefab by name: a new entity at `transform` with the prefab's
    /// entities below it. They appear on the next frame, and are rebuilt whenever the
    /// prefab's file is saved again.
    pub fn spawn_prefab(&mut self, name: &str, transform: &Transform) -> Entity {
        // SAFETY: called inside the system; the engine copies the name and the transform.
        Entity(unsafe { (api().spawn_prefab)(self.raw, name.as_ptr(), name.len(), transform) })
    }

    /// Makes `child` a child of `parent` (its transform becomes relative to the parent's),
    /// or a root again with `None`, when this system returns.
    pub fn set_parent(&mut self, child: Entity, parent: Option<Entity>) {
        let parent = parent.map_or(sys::VOXL_ENTITY_NONE, |parent| parent.0);
        // SAFETY: called inside the system.
        unsafe { (api().set_parent)(self.raw, child.0, parent) }
    }

    /// Despawns an entity and everything below it, when this system returns.
    pub fn despawn_tree(&mut self, entity: Entity) {
        // SAFETY: called inside the system.
        unsafe { (api().despawn_tree)(self.raw, entity.0) }
    }

    /// Creates an entity. It can be given components right away; it appears in queries once
    /// this system returns.
    pub fn spawn(&mut self) -> Entity {
        // SAFETY: called inside the system.
        Entity(unsafe { (api().spawn)(self.raw) })
    }

    pub fn despawn(&mut self, entity: Entity) {
        // SAFETY: as above.
        unsafe { (api().despawn)(self.raw, entity.0) }
    }

    /// Adds or replaces a component when this system returns.
    pub fn insert<T: Copy + 'static>(
        &mut self,
        entity: Entity,
        component: Component<T>,
        value: &T,
    ) {
        // SAFETY: `value` is `size_of::<T>()` readable bytes, which the engine copies.
        unsafe {
            (api().insert)(
                self.raw,
                entity.0,
                component.id,
                std::ptr::from_ref(value).cast(),
            )
        }
    }

    pub fn remove<T>(&mut self, entity: Entity, component: Component<T>) {
        // SAFETY: as above.
        unsafe { (api().remove)(self.raw, entity.0, component.id) }
    }
}

/// Writes to the engine's log. Prefer the [`info!`], [`warn!`] and [`error!`] macros.
pub fn log(level: u32, message: &str) {
    // SAFETY: the engine copies the message before returning.
    unsafe { (api().log)(level, message.as_ptr(), message.len()) }
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log($crate::sys::VOXL_LOG_INFO, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log($crate::sys::VOXL_LOG_WARN, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log($crate::sys::VOXL_LOG_ERROR, &format!($($arg)*)) };
}

#[doc(hidden)]
pub unsafe fn __load(
    api: *const sys::VoxlApi,
    app: *mut sys::VoxlApp,
    load: fn(&mut App) -> Result<(), Error>,
) -> i32 {
    if api.is_null() || (*api).abi_version != sys::VOXL_ABI_VERSION {
        return -1;
    }
    API.store(api.cast_mut(), Ordering::Relaxed);
    match catch_unwind(AssertUnwindSafe(|| load(&mut App { raw: app }))) {
        Ok(Ok(())) => 0,
        Ok(Err(err)) => {
            log(
                sys::VOXL_LOG_ERROR,
                &format!("plugin failed to load: {err}"),
            );
            -1
        }
        Err(_) => {
            log(sys::VOXL_LOG_ERROR, "plugin panicked while loading");
            -1
        }
    }
}

#[doc(hidden)]
pub fn __unload() {
    API.store(std::ptr::null_mut(), Ordering::Relaxed);
}

/// Exports the three functions the engine looks for, calling `$load` on every load and reload.
/// `$load` is a `fn(&mut App) -> Result<(), Error>`.
#[macro_export]
macro_rules! export_plugin {
    ($load:path) => {
        #[no_mangle]
        pub extern "C" fn voxl_plugin_abi_version() -> u32 {
            $crate::sys::VOXL_ABI_VERSION
        }

        /// # Safety
        /// Called by the engine with its API table and the app being set up.
        #[no_mangle]
        pub unsafe extern "C" fn voxl_plugin_load(
            api: *const $crate::sys::VoxlApi,
            app: *mut $crate::sys::VoxlApp,
        ) -> i32 {
            $crate::__load(api, app, $load)
        }

        #[no_mangle]
        pub extern "C" fn voxl_plugin_unload() {
            $crate::__unload()
        }
    };
}
