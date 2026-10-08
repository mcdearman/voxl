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
