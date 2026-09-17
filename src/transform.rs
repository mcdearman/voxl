use glam::{Mat3, Mat4, Quat, Vec3};

use crate::{
    app::{App, Plugin, Stage},
    ecs::{Commands, Component, Entity, Query, With, Without},
};

/// Local position, rotation and scale. Relative to `Parent` if the entity has one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Component for Transform {}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_xyz(x: f32, y: f32, z: f32) -> Self {
        Self::from_translation(Vec3::new(x, y, z))
    }

    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Rotates so that `forward()` points at `target`.
    pub fn looking_at(mut self, target: Vec3, up: Vec3) -> Self {
        let forward = (target - self.translation).normalize();
        let right = forward.cross(up).normalize();
        let up = right.cross(forward);
        self.rotation = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
        self
    }

    /// Local -Z, the conventional "forward" in a right-handed, Y-up world.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// World-space matrix, computed from `Transform` and the `Parent` chain during `PostUpdate`.
/// Added automatically to anything with a `Transform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform(pub Mat4);

impl Component for GlobalTransform {}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self(Mat4::IDENTITY)
    }
}

impl GlobalTransform {
    pub fn translation(&self) -> Vec3 {
        self.0.w_axis.truncate()
    }

    pub fn forward(&self) -> Vec3 {
        self.0.transform_vector3(Vec3::NEG_Z).normalize()
    }
}

/// Makes this entity's `Transform` relative to another entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

impl Component for Parent {}

const MAX_DEPTH: usize = 64;

fn add_global_transforms(
    mut commands: Commands,
    query: Query<Entity, (With<Transform>, Without<GlobalTransform>)>,
) {
    for entity in &query {
        commands.entity(entity).insert(GlobalTransform::default());
    }
}

/// Walks up the parent chain for every entity. Simple and O(n × depth); a real hierarchy would
/// walk down from the roots and skip unchanged subtrees.
fn propagate_transforms(
    mut globals: Query<(Entity, &mut GlobalTransform)>,
    locals: Query<(&Transform, Option<&Parent>)>,
) {
    for (entity, mut global) in &mut globals {
        let mut matrix = Mat4::IDENTITY;
        let mut current = Some(entity);
        for _ in 0..MAX_DEPTH {
            let Some((transform, parent)) = current.and_then(|e| locals.get(e)) else {
                break;
            };
            matrix = transform.matrix() * matrix;
            current = parent.map(|p| p.0);
        }
        // Only write on change so `Changed<GlobalTransform>` stays meaningful.
        if global.0 != matrix {
            global.0 = matrix;
        }
    }
}

pub struct TransformPlugin;

impl Plugin for TransformPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Stage::PostUpdate,
            (add_global_transforms, propagate_transforms),
        );
    }
}
