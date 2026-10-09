//! The raw interface, exactly as `include/mira.h` declares it. The engine builds its side
//! from these same definitions.

use std::ffi::c_void;

pub const MIRA_ABI_VERSION: u32 = 1;

/// Returned by the optional `mira_plugin_flags`: never unmap this library.
pub const MIRA_PLUGIN_KEEP_LOADED: u32 = 1;

pub type MiraEntity = u64;
pub const MIRA_ENTITY_NONE: MiraEntity = u64::MAX;

pub type MiraComponent = u32;

#[repr(C)]
pub struct MiraApp {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct MiraSystem {
    _opaque: [u8; 0],
}

pub const MIRA_STAGE_STARTUP: u32 = 0;
pub const MIRA_STAGE_FIRST: u32 = 1;
pub const MIRA_STAGE_PRE_UPDATE: u32 = 2;
pub const MIRA_STAGE_FIXED_UPDATE: u32 = 3;
pub const MIRA_STAGE_UPDATE: u32 = 4;
pub const MIRA_STAGE_POST_UPDATE: u32 = 5;
pub const MIRA_STAGE_LAST: u32 = 6;

pub const MIRA_READ: u32 = 0;
pub const MIRA_WRITE: u32 = 1;
pub const MIRA_WITH: u32 = 2;
pub const MIRA_WITHOUT: u32 = 3;

pub const MIRA_KEY_A: u32 = 0;
pub const MIRA_KEY_Z: u32 = 25;
pub const MIRA_KEY_0: u32 = 26;
pub const MIRA_KEY_9: u32 = 35;
pub const MIRA_KEY_SPACE: u32 = 36;
pub const MIRA_KEY_ENTER: u32 = 37;
pub const MIRA_KEY_ESCAPE: u32 = 38;
pub const MIRA_KEY_TAB: u32 = 39;
pub const MIRA_KEY_BACKSPACE: u32 = 40;
pub const MIRA_KEY_LEFT: u32 = 41;
pub const MIRA_KEY_RIGHT: u32 = 42;
pub const MIRA_KEY_UP: u32 = 43;
pub const MIRA_KEY_DOWN: u32 = 44;
pub const MIRA_KEY_LEFT_SHIFT: u32 = 45;
pub const MIRA_KEY_RIGHT_SHIFT: u32 = 46;
pub const MIRA_KEY_LEFT_CONTROL: u32 = 47;
pub const MIRA_KEY_RIGHT_CONTROL: u32 = 48;
pub const MIRA_KEY_LEFT_ALT: u32 = 49;
pub const MIRA_KEY_RIGHT_ALT: u32 = 50;
pub const MIRA_KEY_F1: u32 = 51;
pub const MIRA_KEY_F12: u32 = 62;

pub const MIRA_MOUSE_LEFT: u32 = 0;
pub const MIRA_MOUSE_RIGHT: u32 = 1;
pub const MIRA_MOUSE_MIDDLE: u32 = 2;

pub type MiraMesh = u32;

pub const MIRA_SHAPE_CUBE: u32 = 0;
pub const MIRA_SHAPE_SPHERE: u32 = 1;
pub const MIRA_SHAPE_PLANE: u32 = 2;

pub const MIRA_SIGNAL_AND: u32 = 0;
pub const MIRA_SIGNAL_OR: u32 = 1;
pub const MIRA_SIGNAL_NOT: u32 = 2;
pub const MIRA_SIGNAL_COUNT: u32 = 3;
pub const MIRA_SIGNAL_SUM: u32 = 4;
pub const MIRA_SIGNAL_SELECT: u32 = 5;
pub const MIRA_SIGNAL_TIMER: u32 = 6;
pub const MIRA_SIGNAL_HELD_FOR: u32 = 7;
pub const MIRA_SIGNAL_LESS: u32 = 8;
pub const MIRA_SIGNAL_LESS_OR_EQUAL: u32 = 9;
pub const MIRA_SIGNAL_EQUAL: u32 = 10;
pub const MIRA_SIGNAL_GREATER_OR_EQUAL: u32 = 11;
pub const MIRA_SIGNAL_GREATER: u32 = 12;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraMaterial {
    pub color: [f32; 4],
    pub emissive: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
}

pub const MIRA_FIELD_F32: u32 = 0;
pub const MIRA_FIELD_F64: u32 = 1;
pub const MIRA_FIELD_I32: u32 = 2;
pub const MIRA_FIELD_I64: u32 = 3;
pub const MIRA_FIELD_U8: u32 = 4;
pub const MIRA_FIELD_U32: u32 = 5;
pub const MIRA_FIELD_BOOL: u32 = 6;
pub const MIRA_FIELD_ENTITY: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MiraField {
    pub name: *const u8,
    pub name_len: usize,
    pub field_type: u32,
    pub count: u32,
    pub offset: usize,
}

pub type MiraImage = u32;

pub type MiraEvent = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraCamera {
    pub fov_y: f32,
    pub near: f32,
    pub active: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub shadows: u32,
}

pub const MIRA_COLLIDER_SPHERE: u32 = 0;
pub const MIRA_COLLIDER_BOX: u32 = 1;
pub const MIRA_COLLIDER_CAPSULE: u32 = 2;
pub const MIRA_COLLIDER_GROUND: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraCollider {
    pub shape: u32,
    pub size: [f32; 3],
    pub friction: f32,
    pub restitution: f32,
    pub sensor: u32,
}

pub const MIRA_BODY_DYNAMIC: u32 = 0;
pub const MIRA_BODY_KINEMATIC: u32 = 1;
pub const MIRA_BODY_ANIMATED: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraBody {
    pub kind: u32,
    pub mass: f32,
    pub velocity: [f32; 3],
    pub lock_rotation: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraRayHit {
    pub entity: MiraEntity,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub distance: f32,
}

/// The engine's `mira.Contact` event.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraContact {
    pub a: MiraEntity,
    pub b: MiraEntity,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub impulse: f32,
    pub _pad: u32,
}

const _: () = assert!(std::mem::size_of::<MiraContact>() == 48);

pub const MIRA_LOG_ERROR: u32 = 1;
pub const MIRA_LOG_WARN: u32 = 2;
pub const MIRA_LOG_INFO: u32 = 3;
pub const MIRA_LOG_DEBUG: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiraTerm {
    pub component: MiraComponent,
    pub access: u32,
}

pub type MiraSystemFn = unsafe extern "C" fn(system: *mut MiraSystem, user: *mut c_void);
pub type MiraDropFn = unsafe extern "C" fn(value: *mut c_void);

#[repr(C)]
pub struct MiraSystemDesc {
    pub name: *const u8,
    pub name_len: usize,
    pub stage: u32,
    pub reserved: u32,
    pub run: Option<MiraSystemFn>,
    pub user: *mut c_void,
    pub terms: *const MiraTerm,
    pub term_count: usize,
}

#[repr(C)]
pub struct MiraApi {
    pub abi_version: u32,
    pub size: u32,

    pub log: unsafe extern "C" fn(level: u32, message: *const u8, len: usize),

    pub component_register: unsafe extern "C" fn(
        app: *mut MiraApp,
        name: *const u8,
        name_len: usize,
        size: usize,
        align: usize,
        drop: Option<MiraDropFn>,
    ) -> MiraComponent,
    pub component_lookup: unsafe extern "C" fn(
        app: *mut MiraApp,
        name: *const u8,
        name_len: usize,
        size: *mut usize,
        align: *mut usize,
    ) -> MiraComponent,
    pub state: unsafe extern "C" fn(
        app: *mut MiraApp,
        name: *const u8,
        name_len: usize,
        size: usize,
        align: usize,
    ) -> *mut c_void,
    pub system_add: unsafe extern "C" fn(app: *mut MiraApp, desc: *const MiraSystemDesc) -> i32,

    pub delta_seconds: unsafe extern "C" fn(system: *mut MiraSystem) -> f32,
    pub elapsed_seconds: unsafe extern "C" fn(system: *mut MiraSystem) -> f64,
    pub query_next: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: *mut MiraEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_get: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub spawn: unsafe extern "C" fn(system: *mut MiraSystem) -> MiraEntity,
    pub despawn: unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity),
    pub insert: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        component: MiraComponent,
        value: *const c_void,
    ),
    pub remove:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, component: MiraComponent),

    pub system_add_query: unsafe extern "C" fn(
        app: *mut MiraApp,
        system: *const u8,
        system_len: usize,
        terms: *const MiraTerm,
        term_count: usize,
    ) -> i32,

    pub query_next_in: unsafe extern "C" fn(
        system: *mut MiraSystem,
        query: u32,
        entity: *mut MiraEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_get_in: unsafe extern "C" fn(
        system: *mut MiraSystem,
        query: u32,
        entity: MiraEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_rewind: unsafe extern "C" fn(system: *mut MiraSystem, query: u32),

    pub key_down: unsafe extern "C" fn(system: *mut MiraSystem, key: u32) -> u8,
    pub key_pressed: unsafe extern "C" fn(system: *mut MiraSystem, key: u32) -> u8,
    pub key_released: unsafe extern "C" fn(system: *mut MiraSystem, key: u32) -> u8,
    pub mouse_down: unsafe extern "C" fn(system: *mut MiraSystem, button: u32) -> u8,
    pub mouse_pressed: unsafe extern "C" fn(system: *mut MiraSystem, button: u32) -> u8,
    pub mouse_motion: unsafe extern "C" fn(system: *mut MiraSystem, delta: *mut f32),

    pub mesh_shape: unsafe extern "C" fn(system: *mut MiraSystem, shape: u32, a: f32) -> MiraMesh,
    pub mesh_create: unsafe extern "C" fn(
        system: *mut MiraSystem,
        vertices: *const MiraVertex,
        vertex_count: usize,
        indices: *const u32,
        index_count: usize,
    ) -> MiraMesh,
    pub set_mesh: unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, mesh: MiraMesh),
    pub set_material: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        material: *const MiraMaterial,
    ),

    pub event_register: unsafe extern "C" fn(
        app: *mut MiraApp,
        name: *const u8,
        name_len: usize,
        size: usize,
    ) -> MiraEvent,
    pub event_send:
        unsafe extern "C" fn(system: *mut MiraSystem, event: MiraEvent, value: *const c_void),
    pub event_next:
        unsafe extern "C" fn(system: *mut MiraSystem, event: MiraEvent, value: *mut c_void) -> u8,

    pub set_camera: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        camera: *const MiraCamera,
    ),
    pub set_light:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, light: *const MiraLight),
    pub set_ambient:
        unsafe extern "C" fn(system: *mut MiraSystem, color: *const f32, intensity: f32),
    pub set_window_title:
        unsafe extern "C" fn(system: *mut MiraSystem, title: *const u8, len: usize),

    pub set_collider: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        collider: *const MiraCollider,
    ),
    pub set_body:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, body: *const MiraBody),
    pub apply_impulse:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, impulse: *const f32),
    pub set_velocity:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, velocity: *const f32),
    pub velocity:
        unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity, velocity: *mut f32) -> u8,
    pub raycast: unsafe extern "C" fn(
        system: *mut MiraSystem,
        origin: *const f32,
        direction: *const f32,
        max_distance: f32,
        hit: *mut MiraRayHit,
    ) -> u8,

    pub component_describe: unsafe extern "C" fn(
        app: *mut MiraApp,
        component: MiraComponent,
        fields: *const MiraField,
        field_count: usize,
    ) -> i32,

    pub image_load:
        unsafe extern "C" fn(system: *mut MiraSystem, name: *const u8, len: usize) -> MiraImage,
    pub set_textures: unsafe extern "C" fn(
        system: *mut MiraSystem,
        entity: MiraEntity,
        base_color: MiraImage,
        normal: MiraImage,
        metallic_roughness: MiraImage,
    ),
    pub spawn_model: unsafe extern "C" fn(
        system: *mut MiraSystem,
        name: *const u8,
        len: usize,
        transform: *const MiraTransform,
    ) -> MiraEntity,

    pub set_parent:
        unsafe extern "C" fn(system: *mut MiraSystem, child: MiraEntity, parent: MiraEntity),
    pub despawn_tree: unsafe extern "C" fn(system: *mut MiraSystem, entity: MiraEntity),

    pub spawn_prefab: unsafe extern "C" fn(
        system: *mut MiraSystem,
        name: *const u8,
        len: usize,
        transform: *const MiraTransform,
    ) -> MiraEntity,

    pub system_fail: unsafe extern "C" fn(
        system: *mut MiraSystem,
        message: *const u8,
        len: usize,
        trace: *const u8,
        trace_len: usize,
    ),

    pub signal_set: unsafe extern "C" fn(
        system: *mut MiraSystem,
        name: *const u8,
        len: usize,
        value: f64,
        number: u32,
    ),
    pub signal_get: unsafe extern "C" fn(
        system: *mut MiraSystem,
        name: *const u8,
        len: usize,
        out: *mut f64,
    ) -> u32,
    pub signal_define: unsafe extern "C" fn(
        system: *mut MiraSystem,
        name: *const u8,
        len: usize,
        op: u32,
        param: f64,
        inputs: *const u8,
        inputs_len: usize,
    ),
}

/// `mira.Transform`: 48 bytes, 16-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraTransform {
    pub translation: [f32; 3],
    pub _pad0: f32,
    pub rotation: MiraQuat,
    pub scale: [f32; 3],
    pub _pad1: f32,
}

/// A unit quaternion as x, y, z, w.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiraQuat(pub [f32; 4]);

const _: () = {
    assert!(std::mem::size_of::<MiraTransform>() == 48);
    assert!(std::mem::align_of::<MiraTransform>() == 16);
    assert!(std::mem::offset_of!(MiraTransform, rotation) == 16);
    assert!(std::mem::offset_of!(MiraTransform, scale) == 32);
};
