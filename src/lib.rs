// Lets the derive macros name this crate as `::voxl` from inside it too.
extern crate self as voxl;

pub mod app;
pub mod asset_server;
pub mod assets;
pub mod ecs;
pub mod input;
pub mod physics;
pub mod plugin;
pub mod prefab;
pub mod reflect;
pub mod render;
pub mod tasks;
pub mod time;
pub mod transform;
pub mod voxel;
pub mod window;

pub use glam;

pub mod prelude {
    pub use crate::{
        app::{App, AppExit, DefaultPlugins, Plugin, Stage},
        asset_server::AssetServer,
        assets::{Assets, Handle},
        ecs::prelude::*,
        input::{ButtonInput, KeyCode, Mouse, MouseButton},
        physics::{
            fluid::{Emitter, ParticleFluid, WaterSurface},
            BodyKind, CharacterController, Collider, Joint, JointKind, PhysicsPlugin, PhysicsWorld,
            RigidBody,
        },
        prefab::{PrefabInstance, PrefabPlugin, Prefabs},
        reflect::{NotSaved, Reflect, Scene, TypeRegistry},
        render::{
            AmbientLight, Animator, Camera, Color, DirectionalLight, Environment, Fog, Gait,
            GltfScene, Image, Leg, Limb, LodLevel, Lods, Material, Mesh, Mesh3d, NotShadowCaster,
            Pattern, PostProcess, RayTracingSettings, Reach, ShadowSettings, VolumetricLight,
        },
        time::{FixedTime, Time},
        transform::{Children, GlobalTransform, HierarchyCommands, Interpolate, Parent, Transform},
        voxel::{BlockId, BlockRegistry, ChunkViewer, VoxelPlugin, VoxelSettings, VoxelWorld},
        window::{Window, WindowFocused, WindowResized, WindowSettings},
    };
    pub use glam::{EulerRot, IVec3, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
}
