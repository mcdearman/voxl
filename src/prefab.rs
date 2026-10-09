//! Prefabs: scenes spawned as many times as wanted.
//!
//! A prefab is a scene with a name: a file (`res/prefabs/torch.json`, saved with
//! `Scene::capture_tree`) or a scene made in code. Give an entity a [`PrefabInstance`] and the
//! prefab's entities appear below it in the hierarchy, so the instance's `Transform` places
//! the whole thing. A prefab can hold instances of other prefabs.
//!
//! Prefab files are watched: save one again and every instance of it is rebuilt in place.
//! What an instance spawned is marked [`NotSaved`], so saving a level saves the instance and
//! not its contents, and loading the level builds them from the prefab as it is then.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    app::{App, Plugin, Stage},
    asset_server::{stamp, AssetServer, Stamp},
    ecs::{Component, Entity, World},
    reflect::{NotSaved, Reflect, Scene, TypeRegistry, Value},
    transform::{despawn_recursive, Parent},
};

/// How deep prefabs inside prefabs are spawned in one frame; deeper ones follow next frame.
const MAX_DEPTH: usize = 8;

struct Entry {
    /// Where the prefab was read from, if it came from a file.
    path: Option<PathBuf>,
    /// None when the file couldn't be read.
    scene: Option<Arc<Scene>>,
    /// Goes up every time the prefab changes; never 0.
    version: u32,
    current: Option<Stamp>,
    /// A newer version of the file seen on the last check, to see whether it is still changing.
    settling: Option<Stamp>,
}

/// Every prefab by name. A resource.
pub struct Prefabs {
    entries: HashMap<String, Entry>,
    last_check: Option<Instant>,
    /// How often prefab files are checked for changes.
    pub check_interval: Duration,
    /// Whether changed prefab files are reloaded. On in development builds.
    pub hot_reload: bool,
}

impl Default for Prefabs {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            last_check: None,
            check_interval: Duration::from_millis(250),
            hot_reload: cfg!(debug_assertions),
        }
    }
}

impl Prefabs {
    /// Defines a prefab in code, or replaces one. Its instances are rebuilt.
    pub fn insert(&mut self, name: impl Into<String>, scene: Scene) {
        let name = name.into();
        let version = self.entries.get(&name).map_or(1, |entry| entry.version + 1);
        self.entries.insert(
            name,
            Entry {
                path: None,
                scene: Some(Arc::new(scene)),
                version,
                current: None,
                settling: None,
            },
        );
    }

    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.entries.get(name)?.scene.as_deref()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Reads a prefab file, if it isn't loaded already. A file that can't be read is
    /// reported once, and tried again when it changes.
    pub fn load(&mut self, name: &str, path: PathBuf) {
        if self.entries.contains_key(name) {
            return;
        }
        let entry = Entry {
            current: stamp(&path),
            scene: read(&path),
            path: Some(path),
            version: 1,
            settling: None,
        };
        self.entries.insert(name.to_owned(), entry);
    }

    /// Reloads every prefab whose file has changed, right now, without waiting for the file
    /// to settle. Returns how many were reloaded.
    pub fn reload_changed(&mut self) -> usize {
        let mut reloaded = 0;
        for entry in self.entries.values_mut() {
            let now = entry.path.as_deref().and_then(stamp);
            if now.is_some() && now != entry.current {
                entry.reload();
                reloaded += 1;
            }
        }
        reloaded
    }

    fn check_files(&mut self) {
        if !self.hot_reload
            || self
                .last_check
                .is_some_and(|t| t.elapsed() < self.check_interval)
        {
            return;
        }
        self.last_check = Some(Instant::now());
        for entry in self.entries.values_mut() {
            let now = entry.path.as_deref().and_then(stamp);
            if now.is_none() || now == entry.current {
                entry.settling = None;
            } else if now == entry.settling {
                // Unchanged since the last check: whatever was writing it has finished.
                entry.reload();
            } else {
                entry.settling = now;
            }
        }
    }
}

impl Entry {
    fn reload(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        self.current = stamp(path);
        self.settling = None;
        // A file that no longer reads keeps the last good version in use.
        if let Some(scene) = read(path) {
            self.scene = Some(scene);
            self.version += 1;
            log::info!("reloaded {}", path.display());
        }
    }
}

fn read(path: &std::path::Path) -> Option<Arc<Scene>> {
    match Scene::load(path) {
        Ok(scene) => Some(Arc::new(scene)),
        Err(err) => {
            log::error!("{err:#}");
            None
        }
    }
}

/// One difference between an instance and its prefab: a field of a component of one of the
/// prefab's entities, set to another value.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(name = "voxl.PrefabOverride")]
pub struct PrefabOverride {
    /// Which of the prefab's entities, counting from 0 in the prefab's order.
    pub entity: u32,
    /// The component's name, as in a scene file: `voxl.Transform`.
    pub component: String,
    /// The field to set, as a path (`translation.1`, `color.r`); empty for the whole
    /// component, which also adds a component the prefab's entity doesn't have.
    pub path: String,
    pub value: Value,
}

/// Puts a prefab's entities below this entity. Give the entity a `Transform` to place them.
#[derive(Clone, Debug, Default, Reflect)]
#[reflect(name = "voxl.PrefabInstance")]
pub struct PrefabInstance {
    /// The prefab's name: a file, relative to the asset server's root, or a name given to
    /// [`Prefabs::insert`].
    pub prefab: String,
    /// Where this instance differs from the prefab. Applied again whenever the instance is
    /// rebuilt, so they survive changes to the prefab. Change them with
    /// [`PrefabInstance::set_override`].
    #[reflect(default)]
    pub overrides: Vec<PrefabOverride>,
    /// The version of the prefab that is spawned; 0 before any is.
    #[reflect(skip)]
    version: u32,
    #[reflect(skip)]
    spawned: Vec<Entity>,
}

impl Component for PrefabInstance {}

impl PrefabInstance {
    pub fn new(prefab: impl Into<String>) -> Self {
        Self {
            prefab: prefab.into(),
            overrides: Vec::new(),
            version: 0,
            spawned: Vec::new(),
        }
    }

    /// Sets a field of one of the prefab's entities for this instance only. See
    /// [`PrefabOverride`] for what the arguments mean.
    pub fn with_override(
        mut self,
        entity: u32,
        component: &str,
        path: &str,
        value: impl Reflect,
    ) -> Self {
        self.set_override(entity, component, path, value);
        self
    }

    /// As [`PrefabInstance::with_override`], on an instance that may already be spawned: it
    /// is rebuilt on the next frame.
    pub fn set_override(&mut self, entity: u32, component: &str, path: &str, value: impl Reflect) {
        let value = value.to_value();
        match self
            .overrides
            .iter_mut()
            .find(|o| o.entity == entity && o.component == component && o.path == path)
        {
            Some(existing) => existing.value = value,
            None => self.overrides.push(PrefabOverride {
                entity,
                component: component.to_owned(),
                path: path.to_owned(),
                value,
            }),
        }
        self.version = 0;
    }

    /// The entities spawned for this instance, in the prefab's order. Empty until the frame
    /// after the instance is made.
    pub fn entities(&self) -> &[Entity] {
        &self.spawned
    }
}

/// Whether `instance`, an instance of `prefab`, sits inside another instance of it.
fn inside_itself(world: &World, instance: Entity, prefab: &str) -> bool {
    let mut at = instance;
    // Bounded, in case the hierarchy itself has a loop.
    for _ in 0..1024 {
        let Some(parent) = world.get::<Parent>(at) else {
            return false;
        };
        at = parent.0;
        if world
            .get::<PrefabInstance>(at)
            .is_some_and(|outer| outer.prefab == prefab)
        {
            return true;
        }
    }
    false
}

/// Spawns the contents of new instances, and rebuilds instances whose prefab has changed.
pub fn update_prefabs(world: &mut World) {
    if !world.contains_resource::<Prefabs>() || !world.contains_resource::<TypeRegistry>() {
        return;
    }
    world.resource_scope(|world, prefabs: &mut Prefabs| {
        prefabs.check_files();
        world.resource_scope(|world, registry: &mut TypeRegistry| {
            // Each pass spawns what the pass before it brought in: prefabs inside prefabs.
            for _ in 0..MAX_DEPTH {
                let stale: Vec<(Entity, String)> = world
                    .query::<(Entity, &PrefabInstance)>()
                    .iter()
                    .filter(|(_, instance)| {
                        prefabs
                            .entries
                            .get(&instance.prefab)
                            .is_none_or(|entry| entry.version != instance.version)
                    })
                    .map(|(entity, instance)| (entity, instance.prefab.clone()))
                    .collect();
                if stale.is_empty() {
                    break;
                }
                for (entity, name) in stale {
                    if !prefabs.entries.contains_key(&name) {
                        let path = match world.get_resource::<AssetServer>() {
                            Some(server) => server.path(&name),
                            None => PathBuf::from(&name),
                        };
                        prefabs.load(&name, path);
                    }
                    let entry = &prefabs.entries[&name];
                    rebuild(world, registry, entity, &name, entry);
                }
            }
        });
    });
}

fn rebuild(world: &mut World, registry: &TypeRegistry, entity: Entity, name: &str, entry: &Entry) {
    let (old, overrides) = world
        .get_mut::<PrefabInstance>(entity)
        .map(|instance| {
            instance.version = entry.version;
            (
                std::mem::take(&mut instance.spawned),
                instance.overrides.clone(),
            )
        })
        .unwrap_or_default();
    for spawned in old {
        despawn_recursive(world, spawned);
    }
    let Some(scene) = &entry.scene else {
        return;
    };
    if inside_itself(world, entity, name) {
        log::error!("the prefab `{name}` contains itself; the inner one is left empty");
        return;
    }
    let spawned = scene.spawn(world, registry);
    for problem in &spawned.skipped {
        log::warn!("prefab `{name}`: {problem}");
    }
    for &new in &spawned.entities {
        world.insert(new, NotSaved);
        if !world.has::<Parent>(new) {
            world.insert(new, Parent(entity));
        }
    }
    for change in &overrides {
        if let Err(why) = apply(world, registry, &spawned.entities, change) {
            log::warn!(
                "prefab `{name}`: can't set `{}` of `{}` on entity {}: {why}",
                change.path,
                change.component,
                change.entity
            );
        }
    }
    if let Some(instance) = world.get_mut::<PrefabInstance>(entity) {
        instance.spawned = spawned.entities;
    }
}

fn apply(
    world: &mut World,
    registry: &TypeRegistry,
    spawned: &[Entity],
    change: &PrefabOverride,
) -> Result<(), String> {
    let &target = spawned
        .get(change.entity as usize)
        .ok_or_else(|| format!("the prefab has {} entities", spawned.len()))?;
    let component = registry
        .get(&change.component)
        .ok_or("no such component is registered")?;
    let value = if change.path.is_empty() {
        change.value.clone()
    } else {
        let mut value = (component.get)(world, target).ok_or("the entity has no such component")?;
        if !value.set_path(&change.path, change.value.clone()) {
            return Err("there is no such field".to_owned());
        }
        value
    };
    (component.insert)(world, target, &value).map_err(|err| err.to_string())
}

pub struct PrefabPlugin;

impl Plugin for PrefabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Prefabs>()
            .register_type::<PrefabInstance>()
            .add_systems(Stage::PreUpdate, update_prefabs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Transform;

    fn world() -> World {
        let mut world = World::new();
        let mut registry = TypeRegistry::default();
        registry.register::<Transform>();
        registry.register::<Parent>();
        registry.register::<PrefabInstance>();
        world.insert_resource(registry);
        world.insert_resource(Prefabs::default());
        world
    }

    /// A lamp: a post with a light on top of it.
    fn lamp(height: f32) -> Scene {
        let mut world = world();
        let post = world.spawn(Transform::IDENTITY);
        world.spawn((Transform::from_xyz(0.0, height, 0.0), Parent(post)));
        world.resource_scope(|world, registry: &mut TypeRegistry| Scene::capture(world, registry))
    }

    fn capture(world: &mut World) -> Scene {
        world.resource_scope(|world, registry: &mut TypeRegistry| Scene::capture(world, registry))
    }

    fn top_of(world: &World, instance: Entity) -> f32 {
        let spawned = world.get::<PrefabInstance>(instance).unwrap().entities();
        world.get::<Transform>(spawned[1]).unwrap().translation.y
    }

    #[test]
    fn an_instance_gets_the_prefabs_entities_below_it() {
        let mut world = world();
        world.resource_mut::<Prefabs>().insert("lamp", lamp(3.0));
        let a = world.spawn((
            Transform::from_xyz(5.0, 0.0, 0.0),
            PrefabInstance::new("lamp"),
        ));
        let b = world.spawn((
            Transform::from_xyz(9.0, 0.0, 0.0),
            PrefabInstance::new("lamp"),
        ));
        update_prefabs(&mut world);

        for instance in [a, b] {
            let spawned = world
                .get::<PrefabInstance>(instance)
                .unwrap()
                .entities()
                .to_vec();
            assert_eq!(spawned.len(), 2);
            assert_eq!(
                world.get::<Parent>(spawned[0]),
                Some(&Parent(instance)),
                "the root hangs off the instance"
            );
            assert_eq!(
                world.get::<Parent>(spawned[1]),
                Some(&Parent(spawned[0])),
                "links inside are kept"
            );
            assert_eq!(top_of(&world, instance), 3.0);
        }
        let count = world.entity_count();
        update_prefabs(&mut world);
        assert_eq!(world.entity_count(), count, "nothing is spawned twice");
    }

    #[test]
    fn changing_a_prefab_rebuilds_its_instances() {
        let mut world = world();
        world.resource_mut::<Prefabs>().insert("lamp", lamp(3.0));
        let instance = world.spawn((Transform::IDENTITY, PrefabInstance::new("lamp")));
        update_prefabs(&mut world);
        let before = world
            .get::<PrefabInstance>(instance)
            .unwrap()
            .entities()
            .to_vec();
        let count = world.entity_count();

        world.resource_mut::<Prefabs>().insert("lamp", lamp(4.5));
        update_prefabs(&mut world);
        assert_eq!(top_of(&world, instance), 4.5);
        assert_eq!(world.entity_count(), count, "the old ones are gone");
        assert!(before.iter().all(|&old| !world.contains_entity(old)));
    }

    #[test]
    fn a_saved_level_holds_the_instance_and_not_its_contents() {
        let mut world = world();
        world.resource_mut::<Prefabs>().insert("lamp", lamp(3.0));
        world.spawn((
            Transform::from_xyz(5.0, 0.0, 0.0),
            PrefabInstance::new("lamp"),
        ));
        update_prefabs(&mut world);
        let level = capture(&mut world);
        assert_eq!(level.entities.len(), 1, "{level:?}");

        // Loaded where the prefab has since been made taller, the level follows it.
        let mut later = self::world();
        later.resource_mut::<Prefabs>().insert("lamp", lamp(6.0));
        let spawned =
            later.resource_scope(|world, registry: &mut TypeRegistry| level.spawn(world, registry));
        assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
        update_prefabs(&mut later);
        assert_eq!(later.entity_count(), 3);
        assert_eq!(top_of(&later, spawned.entities[0]), 6.0);
    }

    #[test]
    fn an_instance_keeps_its_differences_when_the_prefab_changes() {
        let mut world = world();
        world.resource_mut::<Prefabs>().insert("lamp", lamp(3.0));
        let plain = world.spawn((Transform::IDENTITY, PrefabInstance::new("lamp")));
        let odd = world.spawn((
            Transform::IDENTITY,
            PrefabInstance::new("lamp")
                .with_override(1, "voxl.Transform", "scale.0", 2.0f32)
                // Three that can't be applied are reported and change nothing else.
                .with_override(1, "voxl.Transform", "colour", 1.0f32)
                .with_override(7, "voxl.Transform", "scale.0", 2.0f32)
                .with_override(0, "voxl.Transform", "scale", "wide".to_owned()),
        ));
        update_prefabs(&mut world);
        let light = |world: &World, instance| {
            let spawned = world.get::<PrefabInstance>(instance).unwrap().entities();
            *world.get::<Transform>(spawned[1]).unwrap()
        };
        assert_eq!(light(&world, plain).scale.x, 1.0);
        assert_eq!(light(&world, odd).scale.x, 2.0);
        assert_eq!(light(&world, odd).translation.y, 3.0);

        // The prefab changes: the instance follows it, and stays different where it was.
        world.resource_mut::<Prefabs>().insert("lamp", lamp(4.0));
        update_prefabs(&mut world);
        assert_eq!(light(&world, odd).translation.y, 4.0);
        assert_eq!(light(&world, odd).scale.x, 2.0);

        // A difference added later rebuilds the instance, and it is saved with the level.
        world.get_mut::<PrefabInstance>(odd).unwrap().set_override(
            0,
            "voxl.PrefabInstance",
            "",
            PrefabInstance::new("lamp"),
        );
        update_prefabs(&mut world);
        let spawned = world
            .get::<PrefabInstance>(odd)
            .unwrap()
            .entities()
            .to_vec();
        assert!(
            world.has::<PrefabInstance>(spawned[0]),
            "a whole component can be added"
        );
        let level = capture(&mut world);
        let saved = level
            .entities
            .iter()
            .find(|e| e.id == odd.to_bits())
            .unwrap();
        let (_, instance) = saved
            .components
            .iter()
            .find(|(name, _)| name == "voxl.PrefabInstance")
            .unwrap();
        assert_eq!(
            instance.get_path("overrides.0.value"),
            Some(&Value::Float(2.0))
        );
        assert_eq!(
            PrefabInstance::from_value(instance)
                .unwrap()
                .overrides
                .len(),
            5
        );
    }

    #[test]
    fn prefabs_hold_prefabs_but_not_themselves() {
        let mut world = world();
        world.resource_mut::<Prefabs>().insert("lamp", lamp(3.0));
        // A street: two lamps. And a prefab that holds itself, which must not spawn for ever.
        let street = {
            let mut world = self::world();
            let root = world.spawn(Transform::IDENTITY);
            for x in [0.0, 10.0] {
                world.spawn((
                    Transform::from_xyz(x, 0.0, 0.0),
                    Parent(root),
                    PrefabInstance::new("lamp"),
                ));
            }
            capture(&mut world)
        };
        let ouroboros = {
            let mut world = self::world();
            world.spawn((Transform::IDENTITY, PrefabInstance::new("ouroboros")));
            capture(&mut world)
        };
        world.resource_mut::<Prefabs>().insert("street", street);
        world
            .resource_mut::<Prefabs>()
            .insert("ouroboros", ouroboros);

        let instance = world.spawn((Transform::IDENTITY, PrefabInstance::new("street")));
        update_prefabs(&mut world);
        // The instance, the street's three entities, and two entities for each lamp.
        assert_eq!(world.entity_count(), 1 + 3 + 4);
        assert_eq!(despawn_recursive(&mut world, instance), 8);

        world.spawn((Transform::IDENTITY, PrefabInstance::new("ouroboros")));
        for _ in 0..3 {
            update_prefabs(&mut world);
        }
        assert_eq!(world.entity_count(), 2, "one inner instance, left empty");
    }

    #[test]
    fn a_prefab_file_is_made_from_a_subtree_and_reloads_when_saved() {
        let dir = std::env::temp_dir().join(format!("voxl-prefab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build a lamp under some other entity and save only the lamp.
        let mut world = world();
        world.insert_resource(AssetServer::new(&dir));
        let ground = world.spawn(Transform::IDENTITY);
        let post = world.spawn((Transform::from_xyz(7.0, 0.0, 0.0), Parent(ground)));
        let light = world.spawn((Transform::from_xyz(0.0, 3.0, 0.0), Parent(post)));
        let save = |world: &mut World| {
            world.resource_scope(|world, registry: &mut TypeRegistry| {
                let scene = Scene::capture_tree(world, registry, post);
                assert_eq!(scene.entities.len(), 2);
                assert!(scene.entities[0]
                    .components
                    .iter()
                    .all(|(name, _)| name != "voxl.Parent"));
                scene.save(dir.join("lamp.json")).unwrap();
            });
        };
        save(&mut world);

        let instance = world.spawn((Transform::IDENTITY, PrefabInstance::new("lamp.json")));
        let missing = world.spawn((Transform::IDENTITY, PrefabInstance::new("nothing.json")));
        update_prefabs(&mut world);
        assert_eq!(top_of(&world, instance), 3.0);
        assert!(world
            .get::<PrefabInstance>(missing)
            .unwrap()
            .entities()
            .is_empty());

        // Raise the light, save the prefab again, and the instance follows.
        world.get_mut::<Transform>(light).unwrap().translation.y = 12.5;
        save(&mut world);
        // The file's size changed with its contents, so its stamp has too.
        assert_eq!(world.resource_mut::<Prefabs>().reload_changed(), 1);
        update_prefabs(&mut world);
        assert_eq!(top_of(&world, instance), 12.5);

        // A file that no longer reads leaves the last good version in place.
        std::fs::write(dir.join("lamp.json"), "{ not a scene").unwrap();
        assert_eq!(world.resource_mut::<Prefabs>().reload_changed(), 1);
        update_prefabs(&mut world);
        assert_eq!(top_of(&world, instance), 12.5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
