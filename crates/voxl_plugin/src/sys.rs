//! The raw interface, exactly as `include/voxl.h` declares it. The engine builds its side
//! from these same definitions.

use std::ffi::c_void;

pub const VOXL_ABI_VERSION: u32 = 1;

/// Returned by the optional `voxl_plugin_flags`: never unmap this library.
pub const VOXL_PLUGIN_KEEP_LOADED: u32 = 1;

pub type VoxlEntity = u64;
pub const VOXL_ENTITY_NONE: VoxlEntity = u64::MAX;

pub type VoxlComponent = u32;

#[repr(C)]
pub struct VoxlApp {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct VoxlSystem {
    _opaque: [u8; 0],
}

pub const VOXL_STAGE_STARTUP: u32 = 0;
pub const VOXL_STAGE_FIRST: u32 = 1;
pub const VOXL_STAGE_PRE_UPDATE: u32 = 2;
pub const VOXL_STAGE_FIXED_UPDATE: u32 = 3;
pub const VOXL_STAGE_UPDATE: u32 = 4;
pub const VOXL_STAGE_POST_UPDATE: u32 = 5;
pub const VOXL_STAGE_LAST: u32 = 6;

pub const VOXL_READ: u32 = 0;
pub const VOXL_WRITE: u32 = 1;
pub const VOXL_WITH: u32 = 2;
pub const VOXL_WITHOUT: u32 = 3;

pub const VOXL_KEY_A: u32 = 0;
pub const VOXL_KEY_Z: u32 = 25;
pub const VOXL_KEY_0: u32 = 26;
pub const VOXL_KEY_9: u32 = 35;
pub const VOXL_KEY_SPACE: u32 = 36;
pub const VOXL_KEY_ENTER: u32 = 37;
pub const VOXL_KEY_ESCAPE: u32 = 38;
pub const VOXL_KEY_TAB: u32 = 39;
pub const VOXL_KEY_BACKSPACE: u32 = 40;
pub const VOXL_KEY_LEFT: u32 = 41;
pub const VOXL_KEY_RIGHT: u32 = 42;
pub const VOXL_KEY_UP: u32 = 43;
pub const VOXL_KEY_DOWN: u32 = 44;
pub const VOXL_KEY_LEFT_SHIFT: u32 = 45;
pub const VOXL_KEY_RIGHT_SHIFT: u32 = 46;
pub const VOXL_KEY_LEFT_CONTROL: u32 = 47;
pub const VOXL_KEY_RIGHT_CONTROL: u32 = 48;
pub const VOXL_KEY_LEFT_ALT: u32 = 49;
pub const VOXL_KEY_RIGHT_ALT: u32 = 50;
pub const VOXL_KEY_F1: u32 = 51;
pub const VOXL_KEY_F12: u32 = 62;

pub const VOXL_MOUSE_LEFT: u32 = 0;
pub const VOXL_MOUSE_RIGHT: u32 = 1;
pub const VOXL_MOUSE_MIDDLE: u32 = 2;

pub type VoxlMesh = u32;

pub const VOXL_SHAPE_CUBE: u32 = 0;
pub const VOXL_SHAPE_SPHERE: u32 = 1;
pub const VOXL_SHAPE_PLANE: u32 = 2;

pub const VOXL_SIGNAL_AND: u32 = 0;
pub const VOXL_SIGNAL_OR: u32 = 1;
pub const VOXL_SIGNAL_NOT: u32 = 2;
pub const VOXL_SIGNAL_COUNT: u32 = 3;
pub const VOXL_SIGNAL_SUM: u32 = 4;
pub const VOXL_SIGNAL_SELECT: u32 = 5;
pub const VOXL_SIGNAL_TIMER: u32 = 6;
pub const VOXL_SIGNAL_HELD_FOR: u32 = 7;
pub const VOXL_SIGNAL_LESS: u32 = 8;
pub const VOXL_SIGNAL_LESS_OR_EQUAL: u32 = 9;
pub const VOXL_SIGNAL_EQUAL: u32 = 10;
pub const VOXL_SIGNAL_GREATER_OR_EQUAL: u32 = 11;
pub const VOXL_SIGNAL_GREATER: u32 = 12;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlMaterial {
    pub color: [f32; 4],
    pub emissive: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
}

pub const VOXL_FIELD_F32: u32 = 0;
pub const VOXL_FIELD_F64: u32 = 1;
pub const VOXL_FIELD_I32: u32 = 2;
pub const VOXL_FIELD_I64: u32 = 3;
pub const VOXL_FIELD_U8: u32 = 4;
pub const VOXL_FIELD_U32: u32 = 5;
pub const VOXL_FIELD_BOOL: u32 = 6;
pub const VOXL_FIELD_ENTITY: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VoxlField {
    pub name: *const u8,
    pub name_len: usize,
    pub field_type: u32,
    pub count: u32,
    pub offset: usize,
}

pub type VoxlImage = u32;

pub type VoxlEvent = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlCamera {
    pub fov_y: f32,
    pub near: f32,
    pub active: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub shadows: u32,
}

pub const VOXL_COLLIDER_SPHERE: u32 = 0;
pub const VOXL_COLLIDER_BOX: u32 = 1;
pub const VOXL_COLLIDER_CAPSULE: u32 = 2;
pub const VOXL_COLLIDER_GROUND: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlCollider {
    pub shape: u32,
    pub size: [f32; 3],
    pub friction: f32,
    pub restitution: f32,
    pub sensor: u32,
}

pub const VOXL_BODY_DYNAMIC: u32 = 0;
pub const VOXL_BODY_KINEMATIC: u32 = 1;
pub const VOXL_BODY_ANIMATED: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlBody {
    pub kind: u32,
    pub mass: f32,
    pub velocity: [f32; 3],
    pub lock_rotation: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlRayHit {
    pub entity: VoxlEntity,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub distance: f32,
}

/// The engine's `voxl.Contact` event.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlContact {
    pub a: VoxlEntity,
    pub b: VoxlEntity,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub impulse: f32,
    pub _pad: u32,
}

const _: () = assert!(std::mem::size_of::<VoxlContact>() == 48);

pub const VOXL_LOG_ERROR: u32 = 1;
pub const VOXL_LOG_WARN: u32 = 2;
pub const VOXL_LOG_INFO: u32 = 3;
pub const VOXL_LOG_DEBUG: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoxlTerm {
    pub component: VoxlComponent,
    pub access: u32,
}

pub type VoxlSystemFn = unsafe extern "C" fn(system: *mut VoxlSystem, user: *mut c_void);
pub type VoxlDropFn = unsafe extern "C" fn(value: *mut c_void);

#[repr(C)]
pub struct VoxlSystemDesc {
    pub name: *const u8,
    pub name_len: usize,
    pub stage: u32,
    pub reserved: u32,
    pub run: Option<VoxlSystemFn>,
    pub user: *mut c_void,
    pub terms: *const VoxlTerm,
    pub term_count: usize,
}

#[repr(C)]
pub struct VoxlApi {
    pub abi_version: u32,
    pub size: u32,

    pub log: unsafe extern "C" fn(level: u32, message: *const u8, len: usize),

    pub component_register: unsafe extern "C" fn(
        app: *mut VoxlApp,
        name: *const u8,
        name_len: usize,
        size: usize,
        align: usize,
        drop: Option<VoxlDropFn>,
    ) -> VoxlComponent,
    pub component_lookup: unsafe extern "C" fn(
        app: *mut VoxlApp,
        name: *const u8,
        name_len: usize,
        size: *mut usize,
        align: *mut usize,
    ) -> VoxlComponent,
    pub state: unsafe extern "C" fn(
        app: *mut VoxlApp,
        name: *const u8,
        name_len: usize,
        size: usize,
        align: usize,
    ) -> *mut c_void,
    pub system_add: unsafe extern "C" fn(app: *mut VoxlApp, desc: *const VoxlSystemDesc) -> i32,

    pub delta_seconds: unsafe extern "C" fn(system: *mut VoxlSystem) -> f32,
    pub elapsed_seconds: unsafe extern "C" fn(system: *mut VoxlSystem) -> f64,
    pub query_next: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: *mut VoxlEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_get: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub spawn: unsafe extern "C" fn(system: *mut VoxlSystem) -> VoxlEntity,
    pub despawn: unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity),
    pub insert: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        component: VoxlComponent,
        value: *const c_void,
    ),
    pub remove:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, component: VoxlComponent),

    pub system_add_query: unsafe extern "C" fn(
        app: *mut VoxlApp,
        system: *const u8,
        system_len: usize,
        terms: *const VoxlTerm,
        term_count: usize,
    ) -> i32,

    pub query_next_in: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        query: u32,
        entity: *mut VoxlEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_get_in: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        query: u32,
        entity: VoxlEntity,
        components: *mut *mut c_void,
    ) -> u8,
    pub query_rewind: unsafe extern "C" fn(system: *mut VoxlSystem, query: u32),

    pub key_down: unsafe extern "C" fn(system: *mut VoxlSystem, key: u32) -> u8,
    pub key_pressed: unsafe extern "C" fn(system: *mut VoxlSystem, key: u32) -> u8,
    pub key_released: unsafe extern "C" fn(system: *mut VoxlSystem, key: u32) -> u8,
    pub mouse_down: unsafe extern "C" fn(system: *mut VoxlSystem, button: u32) -> u8,
    pub mouse_pressed: unsafe extern "C" fn(system: *mut VoxlSystem, button: u32) -> u8,
    pub mouse_motion: unsafe extern "C" fn(system: *mut VoxlSystem, delta: *mut f32),

    pub mesh_shape: unsafe extern "C" fn(system: *mut VoxlSystem, shape: u32, a: f32) -> VoxlMesh,
    pub mesh_create: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        vertices: *const VoxlVertex,
        vertex_count: usize,
        indices: *const u32,
        index_count: usize,
    ) -> VoxlMesh,
    pub set_mesh: unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, mesh: VoxlMesh),
    pub set_material: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        material: *const VoxlMaterial,
    ),

    pub event_register: unsafe extern "C" fn(
        app: *mut VoxlApp,
        name: *const u8,
        name_len: usize,
        size: usize,
    ) -> VoxlEvent,
    pub event_send:
        unsafe extern "C" fn(system: *mut VoxlSystem, event: VoxlEvent, value: *const c_void),
    pub event_next:
        unsafe extern "C" fn(system: *mut VoxlSystem, event: VoxlEvent, value: *mut c_void) -> u8,

    pub set_camera: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        camera: *const VoxlCamera,
    ),
    pub set_light:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, light: *const VoxlLight),
    pub set_ambient:
        unsafe extern "C" fn(system: *mut VoxlSystem, color: *const f32, intensity: f32),
    pub set_window_title:
        unsafe extern "C" fn(system: *mut VoxlSystem, title: *const u8, len: usize),

    pub set_collider: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        collider: *const VoxlCollider,
    ),
    pub set_body:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, body: *const VoxlBody),
    pub apply_impulse:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, impulse: *const f32),
    pub set_velocity:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, velocity: *const f32),
    pub velocity:
        unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity, velocity: *mut f32) -> u8,
    pub raycast: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        origin: *const f32,
        direction: *const f32,
        max_distance: f32,
        hit: *mut VoxlRayHit,
    ) -> u8,

    pub component_describe: unsafe extern "C" fn(
        app: *mut VoxlApp,
        component: VoxlComponent,
        fields: *const VoxlField,
        field_count: usize,
    ) -> i32,

    pub image_load:
        unsafe extern "C" fn(system: *mut VoxlSystem, name: *const u8, len: usize) -> VoxlImage,
    pub set_textures: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        entity: VoxlEntity,
        base_color: VoxlImage,
        normal: VoxlImage,
        metallic_roughness: VoxlImage,
    ),
    pub spawn_model: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        name: *const u8,
        len: usize,
        transform: *const VoxlTransform,
    ) -> VoxlEntity,

    pub set_parent:
        unsafe extern "C" fn(system: *mut VoxlSystem, child: VoxlEntity, parent: VoxlEntity),
    pub despawn_tree: unsafe extern "C" fn(system: *mut VoxlSystem, entity: VoxlEntity),

    pub spawn_prefab: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        name: *const u8,
        len: usize,
        transform: *const VoxlTransform,
    ) -> VoxlEntity,

    pub system_fail: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        message: *const u8,
        len: usize,
        trace: *const u8,
        trace_len: usize,
    ),

    pub signal_set: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        name: *const u8,
        len: usize,
        value: f64,
        number: u32,
    ),
    pub signal_get: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        name: *const u8,
        len: usize,
        out: *mut f64,
    ) -> u32,
    pub signal_define: unsafe extern "C" fn(
        system: *mut VoxlSystem,
        name: *const u8,
        len: usize,
        op: u32,
        param: f64,
        inputs: *const u8,
        inputs_len: usize,
    ),
}

/// `voxl.Transform`: 48 bytes, 16-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlTransform {
    pub translation: [f32; 3],
    pub _pad0: f32,
    pub rotation: VoxlQuat,
    pub scale: [f32; 3],
    pub _pad1: f32,
}

/// A unit quaternion as x, y, z, w.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxlQuat(pub [f32; 4]);

const _: () = {
    assert!(std::mem::size_of::<VoxlTransform>() == 48);
    assert!(std::mem::align_of::<VoxlTransform>() == 16);
    assert!(std::mem::offset_of!(VoxlTransform, rotation) == 16);
    assert!(std::mem::offset_of!(VoxlTransform, scale) == 32);
};
