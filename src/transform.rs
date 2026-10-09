use glam::{Mat3, Mat4, Quat, Vec3};

use crate::{
    app::{App, Plugin, Stage},
    ecs::{Commands, Component, Entity, EntityCommands, Local, Query, Res, With, Without, World},
    reflect::Reflect,
    relation::{self, Related, Relation},
    time::FixedTime,
};

/// Local position, rotation and scale. Relative to `Parent` if the entity has one.
///
/// Exported to native plugins as `mira.Transform`, so its layout is fixed: it must match
/// `MiraTransform` in `include/mira.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(name = "mira.Transform", default)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Component for Transform {}

/// Whether `Transform` has the layout `include/mira.h` promises. It does wherever glam uses
/// SIMD for quaternions (every desktop target); elsewhere the component isn't exported.
const TRANSFORM_MATCHES_HEADER: bool = {
    use mira_plugin::sys::MiraTransform;
    size_of::<Transform>() == size_of::<MiraTransform>()
        && align_of::<Transform>() == align_of::<MiraTransform>()
        && std::mem::offset_of!(Transform, rotation)
            == std::mem::offset_of!(MiraTransform, rotation)
        && std::mem::offset_of!(Transform, scale) == std::mem::offset_of!(MiraTransform, scale)
};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
#[reflect(name = "mira.Parent")]
pub struct Parent(pub Entity);

impl Component for Parent {}

impl Relation for Parent {
    fn target(&self) -> Entity {
        self.0
    }
}

/// Smooths an entity that is moved in `FixedUpdate`. Each frame its `Transform` is set to a blend
/// of the last two fixed steps, so motion stays smooth when the display and simulation rates
/// differ. The true simulated value is restored before every fixed step.
///
/// Only move such entities from the fixed stages, or call `Interpolate::reset` after teleporting
/// them; writes from `Update` are overwritten.
#[derive(Clone, Copy, Debug, Default, Reflect)]
#[reflect(name = "mira.Interpolate")]
pub struct Interpolate {
    /// `(previous, current)` simulated transforms. `None` until the first fixed step.
    #[reflect(skip)]
    steps: Option<(Transform, Transform)>,
}

impl Component for Interpolate {}

impl Interpolate {
    /// Forgets the recorded steps so the next one doesn't blend from the old location.
    pub fn reset(&mut self) {
        self.steps = None;
    }
}

fn restore_simulated_transforms(mut query: Query<(&mut Transform, &Interpolate)>) {
    for (mut transform, interpolate) in &mut query {
        if let Some((_, current)) = interpolate.steps {
            *transform = current;
        }
    }
}

fn record_simulated_transforms(mut query: Query<(&Transform, &mut Interpolate)>) {
    for (transform, mut interpolate) in &mut query {
        let previous = interpolate.steps.map_or(*transform, |(_, current)| current);
        interpolate.steps = Some((previous, *transform));
    }
}

fn interpolate_transforms(
    fixed: Option<Res<FixedTime>>,
    mut query: Query<(&mut Transform, &Interpolate)>,
) {
    // Without a fixed timestep there is nothing to blend between.
    let Some(fixed) = fixed else {
        return;
    };
    let t = fixed.overstep_fraction();
    for (mut transform, interpolate) in &mut query {
        if let Some((previous, current)) = interpolate.steps {
            *transform = Transform {
                translation: previous.translation.lerp(current.translation, t),
                rotation: previous.rotation.slerp(current.rotation, t),
                scale: previous.scale.lerp(current.scale, t),
            };
        }
    }
}

/// The entities whose `Parent` is this one, in entity order. The engine keeps it up to date
/// from the `Parent` components every frame; read it, and change `Parent` to change it.
/// `Parent` is a [relation](crate::relation), and this is the way back along it.
pub type Children = Related<Parent>;

/// Despawns an entity and everything below it in the hierarchy. Returns how many entities
/// were despawned.
pub fn despawn_recursive(world: &mut World, entity: Entity) -> usize {
    relation::despawn_with_related::<Parent>(world, entity)
}

/// Hierarchy operations on an entity being commanded.
pub trait HierarchyCommands {
    /// Makes this entity a child of `parent`: its `Transform` becomes relative to it.
    fn set_parent(&mut self, parent: Entity) -> &mut Self;
    /// Makes this entity a root again.
    fn remove_parent(&mut self) -> &mut Self;
    /// Despawns this entity and everything below it.
    fn despawn_recursive(&mut self);
}

impl HierarchyCommands for EntityCommands<'_> {
    fn set_parent(&mut self, parent: Entity) -> &mut Self {
        self.insert(Parent(parent))
    }

    fn remove_parent(&mut self) -> &mut Self {
        self.remove::<Parent>()
    }

    fn despawn_recursive(&mut self) {
        self.add(|world, entity| {
            despawn_recursive(world, entity);
        });
    }
}

fn add_global_transforms(
    mut commands: Commands,
    query: Query<Entity, (With<Transform>, Without<GlobalTransform>)>,
) {
    for entity in &query {
        commands.entity(entity).insert(GlobalTransform::default());
    }
}

/// Works out every world-space matrix by walking down from the roots, so each entity costs
/// one matrix multiply however deep it is. An entity whose parent is gone, or has no
/// transform, is treated as a root.
fn propagate_transforms(
    roots: Query<Entity, (With<GlobalTransform>, Without<Parent>)>,
    parented: Query<(Entity, &Parent), With<GlobalTransform>>,
    mut nodes: Query<(&Transform, &mut GlobalTransform, Option<&Children>)>,
    mut pending: Local<Vec<(Entity, Mat4)>>,
) {
    pending.clear();
    pending.extend(roots.iter().map(|entity| (entity, Mat4::IDENTITY)));
    for (entity, parent) in &parented {
        if !nodes.contains(parent.0) {
            pending.push((entity, Mat4::IDENTITY));
        }
    }
    while let Some((entity, parent_matrix)) = pending.pop() {
        let Some((transform, mut global, children)) = nodes.get_mut(entity) else {
            continue;
        };
        let matrix = parent_matrix * transform.matrix();
        // Only write on change so `Changed<GlobalTransform>` stays meaningful.
        if global.0 != matrix {
            global.0 = matrix;
        }
        if let Some(children) = children {
            pending.extend(children.iter().map(|child| (child, matrix)));
        }
    }
}

pub struct TransformPlugin;

impl Plugin for TransformPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Transform>()
            .register_type::<Parent>()
            .register_type::<Interpolate>();
        if TRANSFORM_MATCHES_HEADER {
            app.world.export_component::<Transform>("mira.Transform");
        } else {
            log::warn!("Transform has an unexpected layout here; native plugins can't use it");
        }
        app.add_systems(Stage::FixedFirst, restore_simulated_transforms)
            .add_systems(Stage::FixedLast, record_simulated_transforms)
            .add_systems(
                Stage::PostUpdate,
                (
                    interpolate_transforms,
                    add_global_transforms,
                    relation::sync::<Parent>,
                    propagate_transforms,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::App, ecs::Changed, ecs::ResMut};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(TransformPlugin);
        app
    }

    fn children(app: &App, entity: Entity) -> Option<Vec<Entity>> {
        app.world
            .get::<Children>(entity)
            .map(|c| c.iter().collect())
    }

    fn global(app: &App, entity: Entity) -> Vec3 {
        app.world
            .get::<GlobalTransform>(entity)
            .unwrap()
            .translation()
    }

    #[test]
    fn children_follow_the_parent_links() {
        let mut app = app();
        let parent = app.world.spawn(Transform::IDENTITY);
        let other = app.world.spawn(Transform::IDENTITY);
        let kids: Vec<Entity> = (0..3)
            .map(|_| app.world.spawn((Transform::IDENTITY, Parent(parent))))
            .collect();
        app.update();
        assert_eq!(children(&app, parent), Some(kids.clone()));
        assert_eq!(children(&app, other), None);

        // Moved to another parent.
        app.world.insert(kids[1], Parent(other));
        app.update();
        assert_eq!(children(&app, parent), Some(vec![kids[0], kids[2]]));
        assert_eq!(children(&app, other), Some(vec![kids[1]]));

        // Despawned, and made a root again.
        app.world.despawn(kids[0]);
        app.world.remove::<Parent>(kids[1]);
        app.update();
        assert_eq!(children(&app, parent), Some(vec![kids[2]]));
        assert_eq!(children(&app, other), None, "the last child left");
    }

    #[test]
    fn children_only_change_when_the_links_do() {
        #[derive(Default)]
        struct Changes(usize);
        let mut app = app();
        app.init_resource::<Changes>().add_systems(
            Stage::Last,
            |changed: Query<Entity, Changed<Children>>, mut count: ResMut<Changes>| {
                count.0 += changed.count();
            },
        );
        let parent = app.world.spawn(Transform::IDENTITY);
        let child = app.world.spawn((Transform::IDENTITY, Parent(parent)));
        app.update();
        assert_eq!(app.world.resource::<Changes>().0, 1);
        for _ in 0..5 {
            app.world.get_mut::<Transform>(child).unwrap().translation.x += 1.0;
            app.update();
        }
        assert_eq!(
            app.world.resource::<Changes>().0,
            1,
            "moving things isn't a change of family"
        );
    }

    #[test]
    fn transforms_combine_down_any_depth() {
        let mut app = app();
        let root = app
            .world
            .spawn(Transform::from_xyz(1.0, 0.0, 0.0).with_scale(Vec3::splat(2.0)));
        let child = app
            .world
            .spawn((Transform::from_xyz(0.0, 1.0, 0.0), Parent(root)));
        let grandchild = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 1.0), Parent(child)));
        // A chain much longer than anything real, each link one unit further along X.
        let mut link = app.world.spawn(Transform::IDENTITY);
        let chain_root = link;
        for _ in 0..500 {
            link = app
                .world
                .spawn((Transform::from_xyz(1.0, 0.0, 0.0), Parent(link)));
        }
        app.update();
        assert_eq!(
            global(&app, child),
            Vec3::new(1.0, 2.0, 0.0),
            "scaled by its parent"
        );
        assert_eq!(global(&app, grandchild), Vec3::new(1.0, 2.0, 2.0));
        assert_eq!(global(&app, link), Vec3::new(500.0, 0.0, 0.0));

        // Moving a root moves everything under it, the same frame.
        app.world.get_mut::<Transform>(root).unwrap().translation.x = 10.0;
        app.world
            .get_mut::<Transform>(chain_root)
            .unwrap()
            .translation
            .y = 3.0;
        app.update();
        assert_eq!(global(&app, grandchild), Vec3::new(10.0, 2.0, 2.0));
        assert_eq!(global(&app, link), Vec3::new(500.0, 3.0, 0.0));
    }

    #[test]
    fn an_entity_whose_parent_is_gone_stands_on_its_own() {
        let mut app = app();
        let parent = app.world.spawn(Transform::from_xyz(5.0, 0.0, 0.0));
        let child = app
            .world
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), Parent(parent)));
        let no_place = app.world.spawn_empty();
        let under_nothing = app
            .world
            .spawn((Transform::from_xyz(2.0, 0.0, 0.0), Parent(no_place)));
        app.update();
        assert_eq!(global(&app, child).x, 6.0);
        assert_eq!(
            global(&app, under_nothing).x,
            2.0,
            "a parent with no transform doesn't move it"
        );

        app.world.despawn(parent);
        app.update();
        assert_eq!(global(&app, child).x, 1.0);
    }

    #[test]
    fn despawning_recursively_takes_the_whole_subtree() {
        let mut app = app();
        let root = app.world.spawn(Transform::IDENTITY);
        let branch = app.world.spawn((Transform::IDENTITY, Parent(root)));
        let leaf = app.world.spawn((Transform::IDENTITY, Parent(branch)));
        let bystander = app.world.spawn(Transform::IDENTITY);
        app.update();
        // Added since `Children` was last rebuilt: still part of the tree.
        let late = app.world.spawn((Transform::IDENTITY, Parent(leaf)));

        assert_eq!(despawn_recursive(&mut app.world, branch), 3);
        for gone in [branch, leaf, late] {
            assert!(!app.world.contains_entity(gone));
        }
        assert!(app.world.contains_entity(root) && app.world.contains_entity(bystander));
        app.update();
        assert_eq!(children(&app, root), None);

        // Through commands.
        let child = app.world.spawn((Transform::IDENTITY, Parent(root)));
        app.add_systems(Stage::Update, move |mut commands: Commands| {
            commands.entity(root).despawn_recursive();
        });
        app.update();
        assert!(!app.world.contains_entity(root) && !app.world.contains_entity(child));
        assert_eq!(despawn_recursive(&mut app.world, root), 0, "already gone");
    }

    #[test]
    fn parents_can_be_set_through_commands() {
        let mut app = app();
        let parent = app.world.spawn(Transform::from_xyz(0.0, 4.0, 0.0));
        let child = app.world.spawn(Transform::IDENTITY);
        app.add_systems(
            Stage::Update,
            move |mut commands: Commands, mut done: Local<bool>| {
                if !std::mem::replace(&mut *done, true) {
                    commands.entity(child).set_parent(parent);
                }
            },
        );
        app.update();
        assert_eq!(global(&app, child).y, 4.0);
        assert_eq!(children(&app, parent), Some(vec![child]));

        app.add_systems(Stage::Update, move |mut commands: Commands| {
            commands.entity(child).remove_parent();
        });
        app.update();
        assert_eq!(global(&app, child).y, 0.0);
    }
}
