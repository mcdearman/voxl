// Lets the derive macros name this crate as `::mira` from inside it too.
extern crate self as mira;

pub mod app;
pub mod asset_server;
pub mod assets;
/// The entity component system, which is a crate of its own: `mira_ecs`.
pub use mira_ecs as ecs;
pub mod input;
pub mod live;
pub mod logging;
pub mod mcp;
pub mod physics;
pub mod plugin;
pub mod prefab;
pub mod reflect;
pub mod relation;
pub mod remote;
pub mod render;
pub mod signal;
pub mod state;
pub mod tasks;
pub mod time;
pub mod transform;
pub mod voxel;
pub mod window;

pub use glam;

pub mod prelude {
    pub use crate::{
        app::{App, AppExit, DefaultPlugins, Plugin, Stage},
        asset_server::{AssetServer, Model},
        assets::{Assets, Handle},
        ecs::prelude::*,
        input::{ButtonInput, KeyCode, Mouse, MouseButton},
        live::{Failure, FrameStats, History, Live},
        physics::{
            fluid::{Emitter, ParticleFluid, WaterSurface},
            BodyKind, CharacterController, Collider, Joint, JointKind, PhysicsPlugin, PhysicsWorld,
            RigidBody,
        },
        prefab::{PrefabInstance, PrefabPlugin, Prefabs},
        reflect::{NotSaved, Reflect, Scene, TypeRegistry},
        render::{
            AmbientLight, Animator, Camera, Color, DebugLines, DirectionalLight, Environment, Fog, Gait,
            GltfScene, Image, Leg, Limb, LodLevel, Lods, Material, Mesh, Mesh3d, NotShadowCaster,
            Pattern, PostProcess, RayTracingSettings, Reach, ShadowSettings, VolumetricLight,
        },
        relation::{Related, Relation},
        signal::{
            signal, signal_became_false, signal_became_true, Compare, Op, Signal, SignalChanged,
            SignalPlugin, Signals,
        },
        state::{in_state, NextState, State, States},
        time::{FixedTime, Time},
        transform::{
            Children, GlobalTransform, HierarchyCommands, Interpolate, Name, Parent, Transform,
        },
        voxel::{BlockId, BlockRegistry, ChunkViewer, VoxelPlugin, VoxelSettings, VoxelWorld},
        window::{Window, WindowFocused, WindowResized, WindowSettings},
    };
    pub use glam::{EulerRot, IVec3, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
}
