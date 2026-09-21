pub mod app;
pub mod assets;
pub mod ecs;
pub mod input;
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
        assets::{Assets, Handle},
        ecs::prelude::*,
        input::{ButtonInput, KeyCode, Mouse, MouseButton},
        render::{
            AmbientLight, Camera, ClearColor, Color, DirectionalLight, Fog, Material, Mesh, Mesh3d,
        },
        time::{FixedTime, Time},
        transform::{GlobalTransform, Interpolate, Parent, Transform},
        voxel::{BlockId, BlockRegistry, ChunkViewer, VoxelPlugin, VoxelSettings, VoxelWorld},
        window::{Window, WindowFocused, WindowResized, WindowSettings},
    };
    pub use glam::{EulerRot, IVec3, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
}
