use std::collections::{BTreeMap, HashMap};

use super::{
    json,
    registry::TypeRegistry,
    value::{ReflectError, Value},
};
use crate::ecs::{Entity, World};

/// The format of scene files. Raised when a change would make old readers misread new files.
const VERSION: i64 = 1;

/// Entities and their components as plain data: a level, a prefab, a save.
///
/// Only components registered in the [`TypeRegistry`] are captured. References between
/// entities (a `Parent`, say) are kept, and point at the right new entities when the scene
/// is spawned again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub entities: Vec<SceneEntity>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneEntity {
    /// The entity's id when captured. Only meaningful inside the scene, as what references
    /// between its entities point to.
    pub id: u64,
    /// Component values by type name, in the order the types were registered.
    pub components: Vec<(String, Value)>,
}

/// What happened when a scene was spawned.
#[derive(Debug, Default)]
pub struct Spawned {
    /// The new entities, in the scene's order.
    pub entities: Vec<Entity>,
    /// Components that were left out, with why: an unknown type, a value that no longer
    /// fits its type, a reference to an entity that isn't in the scene.
    pub skipped: Vec<String>,
}

impl Scene {
    /// Captures every entity that has at least one registered component.
    pub fn capture(world: &World, registry: &TypeRegistry) -> Self {
        // Ordered by entity, so the same world always gives the same file.
        let mut entities: BTreeMap<Entity, Vec<(String, Value)>> = BTreeMap::new();
        for component in registry.iter() {
            for entity in (component.entities)(world) {
                if let Some(value) = (component.get)(world, entity) {
                    entities
                        .entry(entity)
                        .or_default()
                        .push((component.name.to_owned(), value));
                }
            }
        }
        Self {
            entities: entities
                .into_iter()
                .map(|(entity, components)| SceneEntity {
                    id: entity.to_bits(),
                    components,
                })
                .collect(),
        }
    }

    /// Creates the scene's entities in a world. Components that can't be restored are
    /// skipped and reported, so one bad value doesn't lose the rest of a level.
    pub fn spawn(&self, world: &mut World, registry: &TypeRegistry) -> Spawned {
        let entities: Vec<Entity> = self.entities.iter().map(|_| world.spawn_empty()).collect();
        let new_ids: HashMap<u64, u64> = self
            .entities
            .iter()
            .zip(&entities)
            .map(|(saved, new)| (saved.id, new.to_bits()))
            .collect();

        let mut skipped = Vec::new();
        for (saved, &entity) in self.entities.iter().zip(&entities) {
            for (name, value) in &saved.components {
                let Some(component) = registry.get(name) else {
                    skipped.push(format!("entity {}: unknown component `{name}`", saved.id));
                    continue;
                };
                // Point references at the new entities. One that leads outside the scene
                // can't be honoured, and must not land on whatever now has that id.
                let mut value = value.clone();
                let mut dangling = false;
                value.for_each_entity(&mut |id| match new_ids.get(id) {
                    Some(new) => *id = *new,
                    None => dangling = true,
                });
                if dangling {
                    skipped.push(format!(
                        "entity {}: `{name}` refers to an entity outside the scene",
                        saved.id
                    ));
                    continue;
                }
                if let Err(err) = (component.insert)(world, entity, &value) {
                    skipped.push(format!("entity {}: `{name}`: {err}", saved.id));
                }
            }
        }
        Spawned { entities, skipped }
    }

    pub fn to_value(&self) -> Value {
        let entities = self
            .entities
            .iter()
            .map(|entity| {
                Value::Map(vec![
                    ("id".to_owned(), Value::Int(entity.id as i64)),
                    (
                        "components".to_owned(),
                        Value::Map(entity.components.clone()),
                    ),
                ])
            })
            .collect();
        Value::Map(vec![
            ("version".to_owned(), Value::Int(VERSION)),
            ("entities".to_owned(), Value::List(entities)),
        ])
    }

    pub fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value.field("version") {
            Some(Value::Int(VERSION)) => {}
            Some(Value::Int(other)) => {
                return Err(ReflectError::new(format!(
                    "this is a version {other} scene, and this engine reads version {VERSION}"
                )))
            }
            _ => return Err(ReflectError::new("not a scene: it has no `version`")),
        }
        let Some(Value::List(items)) = value.field("entities") else {
            return Err(ReflectError::new("the scene has no `entities` list"));
        };
        let entities = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let bad = |what: &str| {
                    ReflectError::new(what)
                        .inside_index(index)
                        .inside("entities")
                };
                let Some(Value::Int(id)) = item.field("id") else {
                    return Err(bad("an entity needs a whole-number `id`"));
                };
                let Some(Value::Map(components)) = item.field("components") else {
                    return Err(bad("an entity needs a `components` map"));
                };
                Ok(SceneEntity {
                    id: *id as u64,
                    components: components.clone(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { entities })
    }

    /// Writes the scene to a file as JSON.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        std::fs::write(path, self.to_json())
            .map_err(|err| anyhow::anyhow!("can't write {}: {err}", path.display()))
    }

    /// Reads a scene from a JSON file.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| anyhow::anyhow!("can't read {}: {err}", path.display()))?;
        Self::from_json(&text).map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))
    }

    pub fn to_json(&self) -> String {
        json::to_string(&self.to_value())
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        Ok(Self::from_value(&json::parse(text)?)?)
    }
}
