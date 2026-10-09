//! The engine's side of `include/mira.h`: the functions a plugin is handed.
//!
//! Every function checks its arguments and catches panics, because whatever is on the other
//! side of the boundary may be in any language and can't be unwound through.

use std::{
    alloc::{self, Layout},
    collections::HashMap,
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::NonNull,
};

use mira_plugin::sys::{self, MiraApi, MiraApp, MiraComponent, MiraEntity, MiraSystem};

use super::{
    events::PluginEvents,
    system::{check_queries, Context, DynamicSystem, Term},
};
use crate::{
    app::Stage,
    asset_server::AssetServer,
    assets::{Assets, Handle},
    ecs::{DropFn, Entity, System, World},
    input::{ButtonInput, KeyCode, Mouse, MouseButton},
    physics::{Collider, PhysicsWorld, RigidBody},
    reflect::{blob_component_type, BlobField, FieldKind, TypeRegistry},
    render::{
        AmbientLight, Camera, Color, DirectionalLight, Image, Material, Mesh, Mesh3d, Vertex,
    },
    signal::{Compare, Op, Signal, Signals},
    transform::{Parent, Transform},
    window::Window,
};

/// A block of plugin state that outlives reloads.
pub(crate) struct StateBlock {
    data: NonNull<u8>,
    layout: Layout,
}

impl StateBlock {
    fn new(layout: Layout) -> Self {
        let data = if layout.size() == 0 {
            NonNull::new(std::ptr::without_provenance_mut(layout.align())).unwrap()
        } else {
            // SAFETY: the layout has a non-zero size.
            NonNull::new(unsafe { alloc::alloc_zeroed(layout) })
                .unwrap_or_else(|| alloc::handle_alloc_error(layout))
        };
        Self { data, layout }
    }
}

impl Drop for StateBlock {
    fn drop(&mut self) {
        if self.layout.size() != 0 {
            // SAFETY: allocated in `new` with this layout.
            unsafe { alloc::dealloc(self.data.as_ptr(), self.layout) }
        }
    }
}

/// What a `MiraApp*` points to while a plugin's load function runs. It collects what the
/// plugin registers; the host installs it once the load has succeeded.
pub(crate) struct Registrar<'a> {
    pub world: &'a mut World,
    pub states: &'a mut HashMap<String, StateBlock>,
    pub plugin: &'a str,
    pub systems: Vec<(Stage, DynamicSystem)>,
    /// Runtime-defined components this plugin registered, by world id.
    pub components: Vec<u32>,
    /// What the plugin said about its systems' order: the system's full name, whether it
    /// runs after (or before) the other, and the other's name.
    pub orders: Vec<(String, bool, String)>,
}

pub(crate) static API: MiraApi = MiraApi {
    abi_version: sys::MIRA_ABI_VERSION,
    size: size_of::<MiraApi>() as u32,
    log,
    component_register,
    component_lookup,
    state,
    system_add,
    delta_seconds,
    elapsed_seconds,
    query_next,
    query_get,
    spawn,
    despawn,
    insert,
    remove,
    system_add_query,
    query_next_in,
    query_get_in,
    query_rewind,
    key_down,
    key_pressed,
    key_released,
    mouse_down,
    mouse_pressed,
    mouse_motion,
    mesh_shape,
    mesh_create,
    set_mesh,
    set_material,
    event_register,
    event_send,
    event_next,
    set_camera,
    set_light,
    set_ambient,
    set_window_title,
    set_collider,
    set_body,
    apply_impulse,
    set_velocity,
    velocity,
    raycast,
    component_describe,
    image_load,
    set_textures,
    spawn_model,
    set_parent,
    despawn_tree,
    spawn_prefab,
    system_fail,
    signal_set,
    signal_get,
    signal_define,
    set_camera_orthographic,
    system_order,
};

/// Runs `body`, turning a panic into `fallback` so it never unwinds into the plugin.
fn guard<T>(what: &str, fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| {
        log::error!(target: "plugin", "the engine panicked in `{what}`");
        fallback
    })
}

/// # Safety
/// `ptr` must be readable for `len` bytes, or null.
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).ok()
}

fn layout_of(size: usize, align: usize) -> Option<Layout> {
    Layout::from_size_align(size, align).ok()
}

/// Component handles are world ids offset by one, so that zero can mean "none".
fn component_id(handle: MiraComponent) -> Option<u32> {
    handle.checked_sub(1)
}

unsafe extern "C" fn log(level: u32, message: *const u8, len: usize) {
    guard("log", (), || {
        let Some(message) = text(message, len) else {
            return;
        };
        let level = match level {
            sys::MIRA_LOG_ERROR => log::Level::Error,
            sys::MIRA_LOG_WARN => log::Level::Warn,
            sys::MIRA_LOG_DEBUG => log::Level::Debug,
            _ => log::Level::Info,
        };
        log::log!(target: "plugin", level, "{message}");
    })
}

unsafe extern "C" fn component_register(
    app: *mut MiraApp,
    name: *const u8,
    name_len: usize,
    size: usize,
    align: usize,
    drop: Option<sys::MiraDropFn>,
) -> MiraComponent {
    guard("component_register", 0, || {
        let Some(registrar) = app.cast::<Registrar>().as_mut() else {
            return 0;
        };
        let (Some(name), Some(layout)) = (text(name, name_len), layout_of(size, align)) else {
            log::error!(target: "plugin", "{}: bad component name or layout", registrar.plugin);
            return 0;
        };
        // SAFETY: the two function types differ only in the pointee of their one argument.
        let drop = drop.map(|f| std::mem::transmute::<sys::MiraDropFn, DropFn>(f));
        match registrar.world.register_blob_component(name, layout, drop) {
            Ok(id) => {
                if !registrar.components.contains(&id) {
                    registrar.components.push(id);
                }
                id + 1
            }
            Err(err) => {
                log::error!(target: "plugin", "{}: {err}", registrar.plugin);
                0
            }
        }
    })
}

unsafe extern "C" fn component_lookup(
    app: *mut MiraApp,
    name: *const u8,
    name_len: usize,
    size: *mut usize,
    align: *mut usize,
) -> MiraComponent {
    guard("component_lookup", 0, || {
        let (Some(registrar), Some(name)) =
            (app.cast::<Registrar>().as_mut(), text(name, name_len))
        else {
            return 0;
        };
        let Some(id) = registrar.world.named_component_id(name) else {
            return 0;
        };
        let layout = registrar.world.named_component(id).unwrap().layout;
        if let Some(size) = size.as_mut() {
            *size = layout.size();
        }
        if let Some(align) = align.as_mut() {
            *align = layout.align();
        }
        id + 1
    })
}

unsafe extern "C" fn state(
    app: *mut MiraApp,
    name: *const u8,
    name_len: usize,
    size: usize,
    align: usize,
) -> *mut c_void {
    guard("state", std::ptr::null_mut(), || {
        let (Some(registrar), Some(name), Some(layout)) = (
            app.cast::<Registrar>().as_mut(),
            text(name, name_len),
            layout_of(size, align),
        ) else {
            return std::ptr::null_mut();
        };
        let block = registrar
            .states
            .entry(name.to_owned())
            .and_modify(|block| {
                if block.layout != layout {
                    log::warn!(
                        target: "plugin",
                        "{}: state `{name}` changed layout and was reset",
                        registrar.plugin
                    );
                    *block = StateBlock::new(layout);
                }
            })
            .or_insert_with(|| StateBlock::new(layout));
        block.data.as_ptr().cast()
    })
}

unsafe extern "C" fn system_add(app: *mut MiraApp, desc: *const sys::MiraSystemDesc) -> i32 {
    guard("system_add", -1, || {
        let (Some(registrar), Some(desc)) = (app.cast::<Registrar>().as_mut(), desc.as_ref())
        else {
            return -1;
        };
        let fail = |why: &str| {
            log::error!(target: "plugin", "{}: system not added: {why}", registrar.plugin);
            -1
        };
        let Some(name) = text(desc.name, desc.name_len) else {
            return fail("its name is missing or not UTF-8");
        };
        let Some(run) = desc.run else {
            return fail("it has no function");
        };
        let stage = match desc.stage {
            sys::MIRA_STAGE_STARTUP => Stage::Startup,
            sys::MIRA_STAGE_FIRST => Stage::First,
            sys::MIRA_STAGE_PRE_UPDATE => Stage::PreUpdate,
            sys::MIRA_STAGE_FIXED_UPDATE => Stage::FixedUpdate,
            sys::MIRA_STAGE_UPDATE => Stage::Update,
            sys::MIRA_STAGE_POST_UPDATE => Stage::PostUpdate,
            sys::MIRA_STAGE_LAST => Stage::Last,
            _ => return fail("unknown stage"),
        };
        if registrar
            .systems
            .iter()
            .any(|(_, s)| crate::ecs::System::name(s) == name)
        {
            return fail("the plugin already has a system with that name");
        }
        let terms = match read_terms(registrar.world, desc.terms, desc.term_count) {
            Ok(terms) => terms,
            Err(why) => return fail(why),
        };
        let name = format!("{}::{name}", registrar.plugin);
        if let Err(conflict) = check_queries(&name, std::slice::from_ref(&terms)) {
            return fail(&conflict);
        }
        let system = DynamicSystem::new(name, stage == Stage::FixedUpdate, run, desc.user, terms);
        registrar.systems.push((stage, system));
        0
    })
}

/// Reads and checks a plugin's term list.
///
/// # Safety
/// `terms` must be readable for `count` terms, or `count` zero.
unsafe fn read_terms(
    world: &World,
    terms: *const sys::MiraTerm,
    count: usize,
) -> Result<Vec<Term>, &'static str> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if terms.is_null() {
        return Err("its terms are missing");
    }
    std::slice::from_raw_parts(terms, count)
        .iter()
        .map(|raw| {
            let named = component_id(raw.component)
                .and_then(|id| world.named_component(id))
                .ok_or("a term names a component that doesn't exist")?;
            if raw.access > sys::MIRA_WITHOUT {
                return Err("a term has an unknown access mode");
            }
            Ok(Term {
                key: named.key,
                access: raw.access,
                name: named.name.clone(),
            })
        })
        .collect()
}

unsafe extern "C" fn system_add_query(
    app: *mut MiraApp,
    system: *const u8,
    system_len: usize,
    terms: *const sys::MiraTerm,
    term_count: usize,
) -> i32 {
    guard("system_add_query", -1, || {
        let Some(registrar) = app.cast::<Registrar>().as_mut() else {
            return -1;
        };
        let plugin = registrar.plugin;
        let fail = |why: &str| {
            log::error!(target: "plugin", "{plugin}: query not added: {why}");
            -1
        };
        let Some(name) = text(system, system_len) else {
            return fail("the system's name is missing or not UTF-8");
        };
        let terms = match read_terms(registrar.world, terms, term_count) {
            Ok(terms) => terms,
            Err(why) => return fail(why),
        };
        let full_name = format!("{plugin}::{name}");
        let Some((_, system)) = registrar
            .systems
            .iter_mut()
            .find(|(_, s)| s.name() == full_name)
        else {
            return fail("add the system before giving it more queries");
        };
        match system.add_query(terms) {
            Ok(index) => index as i32,
            Err(conflict) => fail(&conflict),
        }
    })
}

/// # Safety
/// `system` must be the pointer a running system was called with.
unsafe fn context<'a>(system: *mut MiraSystem) -> Option<&'a mut Context<'a>> {
    system.cast::<Context>().as_mut()
}

unsafe extern "C" fn delta_seconds(system: *mut MiraSystem) -> f32 {
    guard("delta_seconds", 0.0, || {
        context(system).map_or(0.0, |c| c.delta)
    })
}

unsafe extern "C" fn elapsed_seconds(system: *mut MiraSystem) -> f64 {
    guard("elapsed_seconds", 0.0, || {
        context(system).map_or(0.0, |c| c.elapsed)
    })
}

unsafe extern "C" fn query_next(
    system: *mut MiraSystem,
    entity: *mut MiraEntity,
    components: *mut *mut c_void,
) -> u8 {
    guard("query_next", 0, || {
        let Some(context) = context(system) else {
            return 0;
        };
        match context.next(0, components) {
            Some(found) => {
                if let Some(entity) = entity.as_mut() {
                    *entity = found.to_bits();
                }
                1
            }
            None => 0,
        }
    })
}

unsafe extern "C" fn query_get(
    system: *mut MiraSystem,
    entity: MiraEntity,
    components: *mut *mut c_void,
) -> u8 {
    guard("query_get", 0, || {
        context(system).is_some_and(|c| c.get(0, Entity::from_bits(entity), components)) as u8
    })
}

unsafe extern "C" fn spawn(system: *mut MiraSystem) -> MiraEntity {
    guard("spawn", sys::MIRA_ENTITY_NONE, || {
        context(system).map_or(sys::MIRA_ENTITY_NONE, |c| c.spawn().to_bits())
    })
}

unsafe extern "C" fn despawn(system: *mut MiraSystem, entity: MiraEntity) {
    guard("despawn", (), || {
        if let Some(context) = context(system) {
            let entity = Entity::from_bits(entity);
            context.queue.push(move |world| {
                world.despawn(entity);
            });
        }
    })
}

unsafe extern "C" fn insert(
    system: *mut MiraSystem,
    entity: MiraEntity,
    component: MiraComponent,
    value: *const c_void,
) {
    guard("insert", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let named = component_id(component).and_then(|id| context.world().named_component(id));
        let (Some(named), false) = (named, value.is_null()) else {
            log::error!(target: "plugin", "insert: unknown component or null value");
            return;
        };
        // Copy the value now; it is moved into the world when the system's commands apply.
        let bytes = std::slice::from_raw_parts(value.cast::<u8>(), named.layout.size()).to_vec();
        let (key, entity) = (named.key, Entity::from_bits(entity));
        context.queue.push(move |world| {
            // SAFETY: the plugin vouches that these bytes are a value of the component, which
            // is all a value defined across the boundary can ever be.
            unsafe { world.insert_raw(entity, key, bytes.as_ptr()) };
        });
    })
}

unsafe extern "C" fn remove(system: *mut MiraSystem, entity: MiraEntity, component: MiraComponent) {
    guard("remove", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let Some(named) =
            component_id(component).and_then(|id| context.world().named_component(id))
        else {
            return;
        };
        let (key, entity) = (named.key, Entity::from_bits(entity));
        context
            .queue
            .push(move |world| world.remove_by_key(entity, key));
    })
}

unsafe extern "C" fn query_next_in(
    system: *mut MiraSystem,
    query: u32,
    entity: *mut MiraEntity,
    components: *mut *mut c_void,
) -> u8 {
    guard("query_next_in", 0, || {
        let Some(found) = context(system).and_then(|c| c.next(query as usize, components)) else {
            return 0;
        };
        if let Some(entity) = entity.as_mut() {
            *entity = found.to_bits();
        }
        1
    })
}

unsafe extern "C" fn query_get_in(
    system: *mut MiraSystem,
    query: u32,
    entity: MiraEntity,
    components: *mut *mut c_void,
) -> u8 {
    guard("query_get_in", 0, || {
        context(system)
            .is_some_and(|c| c.get(query as usize, Entity::from_bits(entity), components))
            as u8
    })
}

unsafe extern "C" fn query_rewind(system: *mut MiraSystem, query: u32) {
    guard("query_rewind", (), || {
        if let Some(context) = context(system) {
            context.rewind(query as usize);
        }
    })
}

/// The key a `MIRA_KEY_*` number stands for.
fn key_code(key: u32) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    const OTHERS: [KeyCode; 15] = [
        Space,
        Enter,
        Escape,
        Tab,
        Backspace,
        ArrowLeft,
        ArrowRight,
        ArrowUp,
        ArrowDown,
        ShiftLeft,
        ShiftRight,
        ControlLeft,
        ControlRight,
        AltLeft,
        AltRight,
    ];
    const FUNCTION: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    let key = key as usize;
    LETTERS
        .get(key)
        .or_else(|| DIGITS.get(key.wrapping_sub(sys::MIRA_KEY_0 as usize)))
        .or_else(|| OTHERS.get(key.wrapping_sub(sys::MIRA_KEY_SPACE as usize)))
        .or_else(|| FUNCTION.get(key.wrapping_sub(sys::MIRA_KEY_F1 as usize)))
        .copied()
}

fn mouse_button(button: u32) -> Option<MouseButton> {
    match button {
        sys::MIRA_MOUSE_LEFT => Some(MouseButton::Left),
        sys::MIRA_MOUSE_RIGHT => Some(MouseButton::Right),
        sys::MIRA_MOUSE_MIDDLE => Some(MouseButton::Middle),
        _ => None,
    }
}

/// Asks the keyboard state a question. False if there is no keyboard (a headless app).
unsafe fn key(
    system: *mut MiraSystem,
    key: u32,
    ask: fn(&ButtonInput<KeyCode>, KeyCode) -> bool,
) -> u8 {
    guard("key", 0, || {
        let keys = context(system).and_then(|c| c.world().get_resource::<ButtonInput<KeyCode>>());
        keys.zip(key_code(key))
            .is_some_and(|(keys, key)| ask(keys, key)) as u8
    })
}

unsafe extern "C" fn key_down(system: *mut MiraSystem, code: u32) -> u8 {
    key(system, code, ButtonInput::pressed)
}

unsafe extern "C" fn key_pressed(system: *mut MiraSystem, code: u32) -> u8 {
    key(system, code, ButtonInput::just_pressed)
}

unsafe extern "C" fn key_released(system: *mut MiraSystem, code: u32) -> u8 {
    key(system, code, ButtonInput::just_released)
}

unsafe fn mouse(
    system: *mut MiraSystem,
    button: u32,
    ask: fn(&ButtonInput<MouseButton>, MouseButton) -> bool,
) -> u8 {
    guard("mouse", 0, || {
        let buttons =
            context(system).and_then(|c| c.world().get_resource::<ButtonInput<MouseButton>>());
        buttons
            .zip(mouse_button(button))
            .is_some_and(|(buttons, button)| ask(buttons, button)) as u8
    })
}

unsafe extern "C" fn mouse_down(system: *mut MiraSystem, button: u32) -> u8 {
    mouse(system, button, ButtonInput::pressed)
}

unsafe extern "C" fn mouse_pressed(system: *mut MiraSystem, button: u32) -> u8 {
    mouse(system, button, ButtonInput::just_pressed)
}

unsafe extern "C" fn mouse_motion(system: *mut MiraSystem, delta: *mut f32) {
    guard("mouse_motion", (), || {
        if delta.is_null() {
            return;
        }
        let motion = context(system)
            .and_then(|c| c.world().get_resource::<Mouse>())
            .map_or(glam::Vec2::ZERO, |mouse| mouse.delta);
        delta.write(motion.x);
        delta.add(1).write(motion.y);
    })
}

/// Mesh handles are asset ids offset by one, so that zero can mean "none".
fn mesh_handle(context: &mut Context, mesh: Mesh) -> sys::MiraMesh {
    match context.add_mesh(mesh) {
        Some(id) => id + 1,
        None => {
            log::error!(target: "plugin", "this app has no mesh assets (no renderer)");
            0
        }
    }
}

unsafe extern "C" fn mesh_shape(system: *mut MiraSystem, shape: u32, a: f32) -> sys::MiraMesh {
    guard("mesh_shape", 0, || {
        let Some(context) = context(system) else {
            return 0;
        };
        let name = match shape {
            sys::MIRA_SHAPE_CUBE => format!("shape:cube:{a}"),
            sys::MIRA_SHAPE_SPHERE => format!("shape:sphere:{a}"),
            sys::MIRA_SHAPE_PLANE => format!("shape:plane:{a}"),
            _ => {
                log::error!(target: "plugin", "mesh_shape: unknown shape {shape}");
                return 0;
            }
        };
        // Through the asset server when there is one, so the shape has a name; an app without
        // one still gets its mesh.
        if context.world().contains_resource::<AssetServer>() {
            return context.shape_mesh(&name).map_or(0, |id| id + 1);
        }
        let mesh = match shape {
            sys::MIRA_SHAPE_CUBE => Mesh::cube(a),
            sys::MIRA_SHAPE_SPHERE => Mesh::uv_sphere(a, 32, 16),
            _ => Mesh::plane(a),
        };
        mesh_handle(context, mesh)
    })
}

unsafe extern "C" fn mesh_create(
    system: *mut MiraSystem,
    vertices: *const sys::MiraVertex,
    vertex_count: usize,
    indices: *const u32,
    index_count: usize,
) -> sys::MiraMesh {
    guard("mesh_create", 0, || {
        let Some(context) = context(system) else {
            return 0;
        };
        if vertices.is_null() || indices.is_null() || !index_count.is_multiple_of(3) {
            log::error!(target: "plugin", "mesh_create: missing data, or indices not in threes");
            return 0;
        }
        let vertices = std::slice::from_raw_parts(vertices, vertex_count);
        let indices = std::slice::from_raw_parts(indices, index_count);
        if indices.iter().any(|i| *i as usize >= vertices.len()) {
            log::error!(target: "plugin", "mesh_create: an index is past the last vertex");
            return 0;
        }
        let mesh = Mesh {
            vertices: vertices
                .iter()
                .map(|v| Vertex::new(v.position.into(), v.normal.into(), v.uv.into()))
                .collect(),
            indices: indices.to_vec(),
        };
        mesh_handle(context, mesh)
    })
}

unsafe extern "C" fn set_mesh(system: *mut MiraSystem, entity: MiraEntity, mesh: sys::MiraMesh) {
    guard("set_mesh", (), || {
        let (Some(context), Some(id)) = (context(system), mesh.checked_sub(1)) else {
            return;
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            let exists = world
                .get_resource::<Assets<Mesh>>()
                .is_some_and(|meshes| meshes.contains_id(id));
            if exists {
                world.insert(entity, Mesh3d(Handle::from_id(id)));
            } else {
                log::error!(target: "plugin", "set_mesh: no such mesh");
            }
        });
    })
}

unsafe extern "C" fn set_material(
    system: *mut MiraSystem,
    entity: MiraEntity,
    material: *const sys::MiraMaterial,
) {
    guard("set_material", (), || {
        let (Some(context), Some(material)) = (context(system), material.as_ref()) else {
            return;
        };
        let [r, g, b, a] = material.color;
        let [er, eg, eb] = material.emissive;
        let material = Material {
            color: Color { r, g, b, a },
            emissive: Color::rgb(er, eg, eb),
            roughness: material.roughness,
            metallic: material.metallic,
            ..Default::default()
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, material);
        });
    })
}

unsafe extern "C" fn event_register(
    app: *mut MiraApp,
    name: *const u8,
    name_len: usize,
    size: usize,
) -> sys::MiraEvent {
    guard("event_register", 0, || {
        let (Some(registrar), Some(name)) =
            (app.cast::<Registrar>().as_mut(), text(name, name_len))
        else {
            return 0;
        };
        let Some(events) = registrar.world.get_resource_mut::<PluginEvents>() else {
            return 0;
        };
        match events.register(name, size) {
            Ok(id) => id + 1,
            Err(err) => {
                log::error!(target: "plugin", "{}: {err}", registrar.plugin);
                0
            }
        }
    })
}

unsafe extern "C" fn event_send(
    system: *mut MiraSystem,
    event: sys::MiraEvent,
    value: *const c_void,
) {
    guard("event_send", (), || {
        let (Some(context), Some(id)) = (context(system), event.checked_sub(1)) else {
            return;
        };
        let size = context
            .world()
            .get_resource::<PluginEvents>()
            .and_then(|e| e.size(id));
        let Some(size) = size.filter(|size| *size == 0 || !value.is_null()) else {
            log::error!(target: "plugin", "event_send: unknown event or null value");
            return;
        };
        let bytes = if size == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(value.cast::<u8>(), size).to_vec()
        };
        // Sent when the system returns, like its other changes: systems later this frame
        // and every system next frame will find it.
        context.queue.push(move |world| {
            if let Some(events) = world.get_resource_mut::<PluginEvents>() {
                events.send(id, &bytes);
            }
        });
    })
}

unsafe extern "C" fn event_next(
    system: *mut MiraSystem,
    event: sys::MiraEvent,
    value: *mut c_void,
) -> u8 {
    guard("event_next", 0, || {
        let (Some(context), Some(id)) = (context(system), event.checked_sub(1)) else {
            return 0;
        };
        let Some(bytes) = context.next_event(id) else {
            return 0;
        };
        if !bytes.is_empty() {
            if value.is_null() {
                return 0;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), value.cast::<u8>(), bytes.len());
        }
        1
    })
}

unsafe extern "C" fn set_camera(
    system: *mut MiraSystem,
    entity: MiraEntity,
    camera: *const sys::MiraCamera,
) {
    guard("set_camera", (), || {
        let (Some(context), Some(camera)) = (context(system), camera.as_ref()) else {
            return;
        };
        let camera = Camera {
            fov_y: camera.fov_y,
            near: camera.near,
            active: camera.active != 0,
            ..Camera::default()
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, camera);
        });
    })
}

unsafe extern "C" fn set_light(
    system: *mut MiraSystem,
    entity: MiraEntity,
    light: *const sys::MiraLight,
) {
    guard("set_light", (), || {
        let (Some(context), Some(light)) = (context(system), light.as_ref()) else {
            return;
        };
        let [r, g, b] = light.color;
        let light = DirectionalLight {
            color: Color::rgb(r, g, b),
            intensity: light.intensity,
            shadows: light.shadows != 0,
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, light);
        });
    })
}

/// # Safety
/// `values` must be readable for three floats, or null.
unsafe fn vec3(values: *const f32) -> Option<glam::Vec3> {
    (!values.is_null()).then(|| glam::Vec3::from_slice(std::slice::from_raw_parts(values, 3)))
}

unsafe extern "C" fn set_ambient(system: *mut MiraSystem, color: *const f32, intensity: f32) {
    guard("set_ambient", (), || {
        let (Some(context), Some(color)) = (context(system), vec3(color)) else {
            return;
        };
        context.queue.push(move |world| {
            world.insert_resource(AmbientLight {
                color: Color::rgb(color.x, color.y, color.z),
                intensity,
            });
        });
    })
}

unsafe extern "C" fn set_window_title(system: *mut MiraSystem, title: *const u8, len: usize) {
    guard("set_window_title", (), || {
        let (Some(context), Some(title)) = (context(system), text(title, len)) else {
            return;
        };
        // Nothing to do in an app without a window.
        if let Some(window) = context.world().get_resource::<Window>() {
            window.set_title(title);
        }
    })
}

unsafe extern "C" fn set_collider(
    system: *mut MiraSystem,
    entity: MiraEntity,
    collider: *const sys::MiraCollider,
) {
    guard("set_collider", (), || {
        let (Some(context), Some(raw)) = (context(system), collider.as_ref()) else {
            return;
        };
        let [x, y, z] = raw.size;
        let collider = match raw.shape {
            sys::MIRA_COLLIDER_SPHERE => Collider::sphere(x),
            sys::MIRA_COLLIDER_BOX => Collider::cuboid(glam::Vec3::new(x, y, z)),
            sys::MIRA_COLLIDER_CAPSULE => Collider::capsule(y, x),
            sys::MIRA_COLLIDER_GROUND => Collider::ground(),
            other => {
                log::error!(target: "plugin", "set_collider: unknown shape {other}");
                return;
            }
        };
        let mut collider = collider
            .with_friction(raw.friction)
            .with_restitution(raw.restitution);
        if raw.sensor != 0 {
            collider = collider.as_sensor();
        }
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, collider);
        });
    })
}

unsafe extern "C" fn set_body(
    system: *mut MiraSystem,
    entity: MiraEntity,
    body: *const sys::MiraBody,
) {
    guard("set_body", (), || {
        let (Some(context), Some(raw)) = (context(system), body.as_ref()) else {
            return;
        };
        let mut body = match raw.kind {
            sys::MIRA_BODY_DYNAMIC => RigidBody::dynamic(),
            sys::MIRA_BODY_KINEMATIC => RigidBody::kinematic(),
            sys::MIRA_BODY_ANIMATED => RigidBody::animated(),
            other => {
                log::error!(target: "plugin", "set_body: unknown kind {other}");
                return;
            }
        };
        if raw.mass > 0.0 {
            body = body.with_mass(raw.mass);
        }
        body.linear_velocity = raw.velocity.into();
        body.lock_rotation = raw.lock_rotation != 0;
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, body);
        });
    })
}

unsafe extern "C" fn apply_impulse(
    system: *mut MiraSystem,
    entity: MiraEntity,
    impulse: *const f32,
) {
    guard("apply_impulse", (), || {
        let (Some(context), Some(impulse)) = (context(system), vec3(impulse)) else {
            return;
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            if let Some(body) = world.get_mut::<RigidBody>(entity) {
                body.apply_impulse(impulse);
            }
        });
    })
}

unsafe extern "C" fn set_velocity(
    system: *mut MiraSystem,
    entity: MiraEntity,
    velocity: *const f32,
) {
    guard("set_velocity", (), || {
        let (Some(context), Some(velocity)) = (context(system), vec3(velocity)) else {
            return;
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            if let Some(body) = world.get_mut::<RigidBody>(entity) {
                body.linear_velocity = velocity;
                body.wake();
            }
        });
    })
}

unsafe extern "C" fn velocity(system: *mut MiraSystem, entity: MiraEntity, out: *mut f32) -> u8 {
    guard("velocity", 0, || {
        let Some(context) = context(system) else {
            return 0;
        };
        // No plugin can hold a pointer to a body (it isn't a component they can query), so a
        // shared look at it can't alias anything.
        let body = context.world().get::<RigidBody>(Entity::from_bits(entity));
        let (Some(body), false) = (body, out.is_null()) else {
            return 0;
        };
        std::slice::from_raw_parts_mut(out, 3).copy_from_slice(&body.linear_velocity.to_array());
        1
    })
}

unsafe extern "C" fn raycast(
    system: *mut MiraSystem,
    origin: *const f32,
    direction: *const f32,
    max_distance: f32,
    hit: *mut sys::MiraRayHit,
) -> u8 {
    guard("raycast", 0, || {
        let (Some(context), Some(origin), Some(direction)) =
            (context(system), vec3(origin), vec3(direction))
        else {
            return 0;
        };
        let Some(direction) = direction.try_normalize() else {
            return 0;
        };
        let found = context
            .world()
            .get_resource::<PhysicsWorld>()
            .and_then(|physics| physics.raycast(origin, direction, max_distance, None));
        let Some(found) = found else {
            return 0;
        };
        if let Some(hit) = hit.as_mut() {
            *hit = sys::MiraRayHit {
                entity: found.entity.to_bits(),
                point: found.point.into(),
                normal: found.normal.into(),
                distance: found.distance,
            };
        }
        1
    })
}

unsafe extern "C" fn component_describe(
    app: *mut MiraApp,
    component: MiraComponent,
    fields: *const sys::MiraField,
    field_count: usize,
) -> i32 {
    guard("component_describe", -1, || {
        let Some(registrar) = app.cast::<Registrar>().as_mut() else {
            return -1;
        };
        let plugin = registrar.plugin;
        let fail = |why: &str| {
            log::error!(target: "plugin", "{plugin}: component not described: {why}");
            -1
        };
        let named = component_id(component).and_then(|id| registrar.world.named_component(id));
        let Some(named) = named.cloned() else {
            return fail("there is no such component");
        };
        if !matches!(named.key, crate::ecs::ComponentKey::Dynamic(_)) {
            return fail("only a component defined by a plugin can be described");
        }
        if fields.is_null() && field_count > 0 {
            return fail("its fields are missing");
        }
        let raw = if field_count == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(fields, field_count)
        };
        let mut described = Vec::with_capacity(raw.len());
        for field in raw {
            let Some(name) = text(field.name, field.name_len) else {
                return fail("a field's name is missing or not UTF-8");
            };
            let kind = match field.field_type {
                sys::MIRA_FIELD_F32 => FieldKind::F32,
                sys::MIRA_FIELD_F64 => FieldKind::F64,
                sys::MIRA_FIELD_I32 => FieldKind::I32,
                sys::MIRA_FIELD_I64 => FieldKind::I64,
                sys::MIRA_FIELD_U8 => FieldKind::U8,
                sys::MIRA_FIELD_U32 => FieldKind::U32,
                sys::MIRA_FIELD_BOOL => FieldKind::Bool,
                sys::MIRA_FIELD_ENTITY => FieldKind::Entity,
                _ => return fail("a field has an unknown type"),
            };
            described.push(BlobField {
                name: name.to_owned(),
                kind,
                count: field.count as usize,
                offset: field.offset,
            });
        }
        match blob_component_type(&named.name, named.key, named.layout, described) {
            Ok(component) => {
                registrar.world.init_resource::<TypeRegistry>();
                registrar
                    .world
                    .resource_mut::<TypeRegistry>()
                    .insert(component);
                0
            }
            Err(why) => fail(&why),
        }
    })
}

unsafe extern "C" fn image_load(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
) -> sys::MiraImage {
    guard("image_load", 0, || {
        let (Some(context), Some(name)) = (context(system), text(name, len)) else {
            return 0;
        };
        match context.load_image(name) {
            Some(id) => id + 1,
            None => {
                log::error!(target: "plugin", "image_load: this app has no asset server");
                0
            }
        }
    })
}

unsafe extern "C" fn set_textures(
    system: *mut MiraSystem,
    entity: MiraEntity,
    base_color: sys::MiraImage,
    normal: sys::MiraImage,
    metallic_roughness: sys::MiraImage,
) {
    guard("set_textures", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let handle = |image: sys::MiraImage| image.checked_sub(1).map(Handle::<Image>::from_id);
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            let mut material = world.get::<Material>(entity).copied().unwrap_or_default();
            material.base_color_texture = handle(base_color);
            material.normal_texture = handle(normal);
            material.metallic_roughness_texture = handle(metallic_roughness);
            world.insert(entity, material);
        });
    })
}

unsafe extern "C" fn spawn_model(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
    transform: *const sys::MiraTransform,
) -> MiraEntity {
    guard("spawn_model", sys::MIRA_ENTITY_NONE, || {
        let (Some(context), Some(name), Some(at)) =
            (context(system), text(name, len), transform.as_ref())
        else {
            return sys::MIRA_ENTITY_NONE;
        };
        let transform = Transform {
            translation: at.translation.into(),
            rotation: glam::Quat::from_array(at.rotation.0),
            scale: at.scale.into(),
        };
        let root = context.spawn();
        let name = name.to_owned();
        context.queue.push(move |world| {
            if !world.contains_resource::<AssetServer>() {
                log::error!(target: "plugin", "spawn_model: this app has no asset server");
                return;
            }
            // The model file is read here, on the main thread, the first time it is asked for.
            let model = world.resource_scope(|world, server: &mut AssetServer| {
                world.resource_scope(|world, meshes: &mut Assets<Mesh>| {
                    world.resource_scope(|_, images: &mut Assets<Image>| {
                        server.load_gltf(&name, meshes, images)
                    })
                })
            });
            let model = match model {
                Ok(model) => model,
                Err(err) => {
                    log::error!(target: "plugin", "spawn_model: {err:#}");
                    return;
                }
            };
            world.insert(root, transform);
            for part in &model.parts {
                let (scale, rotation, translation) = part.transform.to_scale_rotation_translation();
                world.spawn((
                    Transform {
                        translation,
                        rotation,
                        scale,
                    },
                    Mesh3d(part.mesh),
                    part.material,
                    Parent(root),
                ));
            }
        });
        root.to_bits()
    })
}

unsafe extern "C" fn set_parent(system: *mut MiraSystem, child: MiraEntity, parent: MiraEntity) {
    guard("set_parent", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let child = Entity::from_bits(child);
        context.queue.push(move |world| {
            if parent == sys::MIRA_ENTITY_NONE {
                world.remove::<Parent>(child);
            } else {
                world.insert(child, Parent(Entity::from_bits(parent)));
            }
        });
    })
}

unsafe extern "C" fn despawn_tree(system: *mut MiraSystem, entity: MiraEntity) {
    guard("despawn_tree", (), || {
        if let Some(context) = context(system) {
            let entity = Entity::from_bits(entity);
            context.queue.push(move |world| {
                crate::transform::despawn_recursive(world, entity);
            });
        }
    })
}

unsafe extern "C" fn spawn_prefab(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
    transform: *const sys::MiraTransform,
) -> MiraEntity {
    guard("spawn_prefab", sys::MIRA_ENTITY_NONE, || {
        let (Some(context), Some(name), Some(at)) =
            (context(system), text(name, len), transform.as_ref())
        else {
            return sys::MIRA_ENTITY_NONE;
        };
        let transform = Transform {
            translation: at.translation.into(),
            rotation: glam::Quat::from_array(at.rotation.0),
            scale: at.scale.into(),
        };
        let root = context.spawn();
        let instance = crate::prefab::PrefabInstance::new(name);
        context.queue.push(move |world| {
            world.insert(root, (transform, instance));
        });
        root.to_bits()
    })
}

unsafe extern "C" fn system_fail(
    system: *mut MiraSystem,
    message: *const u8,
    len: usize,
    trace: *const u8,
    trace_len: usize,
) {
    guard("system_fail", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let message = text(message, len).unwrap_or("the plugin gave no reason");
        let trace = text(trace, trace_len).unwrap_or_default();
        // The first failure of a run is the one that matters.
        context
            .failure
            .get_or_insert_with(|| (message.to_owned(), trace.to_owned()));
    })
}

unsafe extern "C" fn signal_set(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
    value: f64,
    number: u32,
) {
    guard("signal_set", (), || {
        let (Some(context), Some(name)) = (context(system), text(name, len)) else {
            return;
        };
        let value = if number != 0 {
            Signal::Number(value)
        } else {
            Signal::Bool(value != 0.0)
        };
        let name = name.to_owned();
        context.queue.push(move |world| {
            if let Some(signals) = world.get_resource_mut::<Signals>() {
                // Left alone when nothing changed, so the graph isn't sorted again every frame.
                if signals.constant(&name) != Some(value) {
                    signals.set(&name, value);
                }
            }
        });
    })
}

unsafe extern "C" fn signal_get(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
    out: *mut f64,
) -> u32 {
    guard("signal_get", 0, || {
        let (Some(context), Some(name)) = (context(system), text(name, len)) else {
            return 0;
        };
        let Some(value) = context
            .world()
            .get_resource::<Signals>()
            .and_then(|signals| signals.get(name))
        else {
            return 0;
        };
        if let Some(out) = out.as_mut() {
            *out = value.number();
        }
        1
    })
}

unsafe extern "C" fn signal_define(
    system: *mut MiraSystem,
    name: *const u8,
    len: usize,
    op: u32,
    param: f64,
    inputs: *const u8,
    inputs_len: usize,
) {
    guard("signal_define", (), || {
        let (Some(context), Some(name)) = (context(system), text(name, len)) else {
            return;
        };
        let op = match op {
            sys::MIRA_SIGNAL_AND => Op::And,
            sys::MIRA_SIGNAL_OR => Op::Or,
            sys::MIRA_SIGNAL_NOT => Op::Not,
            sys::MIRA_SIGNAL_COUNT => Op::Count,
            sys::MIRA_SIGNAL_SUM => Op::Sum,
            sys::MIRA_SIGNAL_SELECT => Op::Select,
            sys::MIRA_SIGNAL_TIMER => Op::Timer,
            sys::MIRA_SIGNAL_HELD_FOR => Op::HeldFor(param),
            sys::MIRA_SIGNAL_LESS => Op::Compare(Compare::Less),
            sys::MIRA_SIGNAL_LESS_OR_EQUAL => Op::Compare(Compare::LessOrEqual),
            sys::MIRA_SIGNAL_EQUAL => Op::Compare(Compare::Equal),
            sys::MIRA_SIGNAL_GREATER_OR_EQUAL => Op::Compare(Compare::GreaterOrEqual),
            sys::MIRA_SIGNAL_GREATER => Op::Compare(Compare::Greater),
            other => {
                log::error!(target: "plugin", "signal_define: there is no operation {other}");
                return;
            }
        };
        let inputs: Vec<String> = text(inputs, inputs_len)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let name = name.to_owned();
        context.queue.push(move |world| {
            if let Some(signals) = world.get_resource_mut::<Signals>() {
                // A plugin defines its rules again every time it loads; the same rule
                // again changes nothing.
                if !signals.is_defined_as(&name, op, &inputs) {
                    signals.define(&name, op, inputs);
                }
            }
        });
    })
}

unsafe extern "C" fn set_camera_orthographic(
    system: *mut MiraSystem,
    entity: MiraEntity,
    height: f32,
    near: f32,
    far: f32,
    active: u32,
) {
    guard("set_camera_orthographic", (), || {
        let Some(context) = context(system) else {
            return;
        };
        let camera = Camera {
            near,
            far,
            active: active != 0,
            ..Camera::orthographic(height)
        };
        let entity = Entity::from_bits(entity);
        context.queue.push(move |world| {
            world.insert(entity, camera);
        });
    })
}

unsafe extern "C" fn system_order(
    app: *mut MiraApp,
    name: *const u8,
    len: usize,
    relation: u32,
    other: *const u8,
    other_len: usize,
) -> i32 {
    guard("system_order", -1, || {
        let Some(registrar) = app.cast::<Registrar>().as_mut() else {
            return -1;
        };
        let (Some(name), Some(other)) = (text(name, len), text(other, other_len)) else {
            return -1;
        };
        let own = |short: &str| format!("{}::{short}", registrar.plugin);
        let is_own = |short: &str| {
            let full = own(short);
            registrar
                .systems
                .iter()
                .any(|(_, system)| crate::ecs::System::name(system) == full)
        };
        if !is_own(name) {
            log::error!(
                target: "plugin",
                "{}: can't order `{name}`: the plugin has added no such system",
                registrar.plugin
            );
            return -1;
        }
        // Another of the plugin's own systems goes by its short name; anything else is
        // named in full.
        let other = if is_own(other) {
            own(other)
        } else {
            other.to_owned()
        };
        registrar
            .orders
            .push((own(name), relation == sys::MIRA_AFTER, other));
        0
    })
}
