use glam::{Quat, Vec3};

use super::{json, Reflect, ReflectError, Scene, Schema, TypeRegistry, Value};
use crate::{
    app::App,
    ecs::{Component, Entity, World},
    render::{Camera, Color, Material},
    transform::{Parent, Transform, TransformPlugin},
};

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(name = "test.Health")]
struct Health {
    current: f32,
    max: f32,
    #[reflect(default)]
    regenerates: bool,
    #[reflect(skip)]
    last_hit_frame: u32,
}

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(default)]
struct Settings {
    name: String,
    volume: f32,
    tags: Vec<String>,
}

#[derive(Reflect, Clone, Debug, PartialEq)]
struct Pair(i32, String);

#[derive(Reflect, Clone, Debug, PartialEq)]
struct Meters(f32);

#[derive(Reflect, Clone, Debug, PartialEq)]
struct Marker;

#[derive(Reflect, Clone, Debug, PartialEq)]
enum Shape {
    Point,
    Sphere(f32),
    Box { half: Vec3, rounded: bool },
    Segment(Vec3, Vec3),
}

fn round_trip<T: Reflect + PartialEq + std::fmt::Debug>(value: T) {
    let plain = value.to_value();
    assert_eq!(
        T::from_value(&plain).as_ref(),
        Ok(&value),
        "through a Value"
    );
    let text = json::to_string(&plain);
    let parsed = json::parse(&text).unwrap_or_else(|err| panic!("{err}\n{text}"));
    assert_eq!(parsed, plain, "through JSON:\n{text}");
}

#[test]
fn derived_types_round_trip() {
    round_trip(Settings {
        name: "quotes \" and \\ and\nnewlines, and ünïcödé 🙂".into(),
        volume: 0.1,
        tags: vec!["a".into(), String::new()],
    });
    round_trip(Pair(-7, "seven".into()));
    round_trip(Meters(1.5e-7));
    round_trip(Marker);
    round_trip(Shape::Point);
    round_trip(Shape::Sphere(2.5));
    round_trip(Shape::Box {
        half: Vec3::new(0.5, 1.0, 0.25),
        rounded: true,
    });
    round_trip(Shape::Segment(Vec3::ZERO, Vec3::ONE));
    round_trip(Some(3u32));
    round_trip(None::<u32>);
    round_trip([Quat::IDENTITY, Quat::from_rotation_y(1.0)]);
    round_trip(Entity::from_bits(0x0000_0007_0000_0003));
    // Floats come back bit for bit, including ones with no short decimal form.
    round_trip(vec![0.1f32, 1.0 / 3.0, f32::MAX, f32::MIN_POSITIVE, -0.0]);
    round_trip(vec![0.1f64, std::f64::consts::PI, 1e300]);
}

#[test]
fn the_plain_form_is_what_you_would_write_by_hand() {
    assert_eq!(
        Meters(2.0).to_value(),
        Value::Float(2.0),
        "a wrapper is what it wraps"
    );
    assert_eq!(Shape::Point.to_value(), Value::Text("Point".into()));
    assert_eq!(
        Shape::Sphere(1.0).to_value(),
        Value::Map(vec![("Sphere".into(), Value::Float(1.0))])
    );
    let text = json::to_string(&Transform::from_xyz(1.0, 2.0, 3.0).to_value());
    assert_eq!(
        text,
        "{\n  \"translation\": [1.0, 2.0, 3.0],\n  \"rotation\": [0.0, 0.0, 0.0, 1.0],\n  \"scale\": [1.0, 1.0, 1.0]\n}\n"
    );
}

#[test]
fn field_attributes() {
    let health = Health {
        current: 3.0,
        max: 10.0,
        regenerates: true,
        last_hit_frame: 99,
    };
    let saved = health.to_value();
    assert!(
        saved.field("last_hit_frame").is_none(),
        "skipped fields aren't saved"
    );
    assert_eq!(
        Health::from_value(&saved).unwrap().last_hit_frame,
        0,
        "and load as the default"
    );

    // A field marked `default` may be missing; any other may not.
    let old_file = json::parse(r#"{"current": 1, "max": 2}"#).unwrap();
    let loaded = Health::from_value(&old_file).unwrap();
    assert_eq!(
        (loaded.current, loaded.max, loaded.regenerates),
        (1.0, 2.0, false)
    );
    let broken = json::parse(r#"{"current": 1}"#).unwrap();
    assert_eq!(
        Health::from_value(&broken).unwrap_err().to_string(),
        "test.Health has no `max`"
    );

    // A type marked `default` takes every missing field from its default value.
    let partial = json::parse(r#"{"volume": 0.5, "unknown": true}"#).unwrap();
    assert_eq!(
        Settings::from_value(&partial).unwrap(),
        Settings {
            volume: 0.5,
            ..Default::default()
        }
    );
}

#[test]
fn errors_say_where() {
    let bad = json::parse(r#"{"Box": {"half": [1, 2, "three"], "rounded": false}}"#).unwrap();
    assert_eq!(
        Shape::from_value(&bad).unwrap_err().to_string(),
        "at `half.2`: expected a number, found text"
    );
    assert!(Shape::from_value(&Value::Text("Blob".into())).is_err());
    assert_eq!(
        u8::from_value(&Value::Int(300)),
        Err(ReflectError::new("300 doesn't fit in u8"))
    );
    let err = json::parse("{\n  \"a\": [1, 2,,]\n}").unwrap_err();
    assert_eq!((err.line, err.column), (2, 14), "{err}");
    assert!(json::parse("[1, 2] trailing").is_err());
    assert!(
        json::parse(&"[".repeat(10_000)).is_err(),
        "deep nesting is refused, not a crash"
    );
}

#[test]
fn json_details() {
    assert_eq!(json::parse("7").unwrap(), Value::Int(7));
    assert_eq!(json::parse("7.0").unwrap(), Value::Float(7.0));
    assert_eq!(json::parse("-2e3").unwrap(), Value::Float(-2000.0));
    assert_eq!(
        json::parse(r#""é🙂\/""#).unwrap(),
        Value::Text("é🙂/".into())
    );
    assert_eq!(json::parse(r#"{"$entity": 5}"#).unwrap(), Value::Entity(5));
    assert_eq!(json::to_string(&Value::Entity(5)), "{\"$entity\": 5}\n");
    // A whole number is accepted where a float is wanted, as hand-written files have them.
    assert_eq!(f32::from_value(&Value::Int(2)), Ok(2.0));
}

#[test]
fn paths_reach_into_values() {
    let mut value = Transform::from_xyz(1.0, 2.0, 3.0).to_value();
    assert_eq!(value.get_path("translation.1"), Some(&Value::Float(2.0)));
    assert!(value.set_path("translation.1", Value::Float(9.0)));
    assert!(!value.set_path("translation.7", Value::Null));
    assert!(!value.set_path("nowhere.x", Value::Null));
    assert_eq!(Transform::from_value(&value).unwrap().translation.y, 9.0);
}

#[test]
fn schemas_describe_shapes() {
    assert_eq!(
        Transform::schema(),
        Schema::Struct {
            name: "mira.Transform",
            fields: Box::new(Schema::Fields(vec![
                ("translation", Schema::Array(Box::new(Schema::Float), 3)),
                ("rotation", Schema::Array(Box::new(Schema::Float), 4)),
                ("scale", Schema::Array(Box::new(Schema::Float), 3)),
            ])),
        }
    );
    let Schema::Enum { variants, .. } = Shape::schema() else {
        panic!("an enum's schema is an enum");
    };
    let names: Vec<_> = variants.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["Point", "Sphere", "Box", "Segment"]);
    assert_eq!(variants[0].1, Schema::Unit);
    assert_eq!(variants[1].1, Schema::Float);
}

fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::default();
    registry.register::<Transform>();
    registry.register::<Parent>();
    registry.register::<Camera>();
    registry.register::<Material>();
    registry.register::<Health>();
    registry
}

#[test]
fn an_inspector_can_edit_a_component_by_name() {
    let registry = registry();
    let mut world = World::new();
    let entity = world.spawn(Transform::IDENTITY);

    let transform = registry.get("mira.Transform").unwrap();
    let mut value = (transform.get)(&world, entity).unwrap();
    value.set_path("scale.0", Value::Float(4.0));
    (transform.insert)(&mut world, entity, &value).unwrap();
    assert_eq!(world.get::<Transform>(entity).unwrap().scale.x, 4.0);

    let health = registry.get("test.Health").unwrap();
    assert!((health.get)(&world, entity).is_none());
    let bad = (health.insert)(&mut world, entity, &Value::Null);
    assert!(
        bad.is_err() && !world.has::<Health>(entity),
        "a bad value changes nothing"
    );
    (transform.remove)(&mut world, entity);
    assert!(!world.has::<Transform>(entity));
}

#[test]
fn a_scene_survives_being_written_and_read_into_another_world() {
    let registry = registry();
    let mut world = World::new();
    world.spawn_empty(); // nothing registered on it: not part of the scene
    let parent = world.spawn((
        Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_rotation_y(0.3)),
        Health {
            current: 5.0,
            max: 8.0,
            regenerates: true,
            last_hit_frame: 4,
        },
    ));
    let child = world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        Parent(parent),
        Material {
            color: Color::rgb(0.2, 0.4, 0.6),
            roughness: 0.3,
            alpha_cutoff: Some(0.5),
            ..Default::default()
        },
    ));
    let camera = world.spawn((Transform::IDENTITY, Camera::default()));

    let text = Scene::capture(&world, &registry).to_json();
    let scene = Scene::from_json(&text).unwrap();
    assert_eq!(
        scene,
        Scene::capture(&world, &registry),
        "the file holds the whole scene"
    );
    assert_eq!(scene.entities.len(), 3);

    // Into a world that already has entities, so every id comes out different.
    let mut other = World::new();
    for _ in 0..10 {
        other.spawn(Transform::IDENTITY);
    }
    let spawned = scene.spawn(&mut other, &registry);
    assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
    let [new_parent, new_child, new_camera] = spawned.entities[..] else {
        panic!("three entities");
    };
    assert_ne!(new_parent, parent);

    assert_eq!(
        other.get::<Transform>(new_parent),
        world.get::<Transform>(parent)
    );
    assert_eq!(
        other.get::<Transform>(new_child),
        world.get::<Transform>(child)
    );
    assert_eq!(
        other.get::<Parent>(new_child),
        Some(&Parent(new_parent)),
        "the reference follows the entity to its new id"
    );
    let health = other.get::<Health>(new_parent).unwrap();
    assert_eq!(
        (health.current, health.max, health.regenerates),
        (5.0, 8.0, true)
    );
    let material = other.get::<Material>(new_child).unwrap();
    assert_eq!(
        (material.color, material.roughness),
        (Color::rgb(0.2, 0.4, 0.6), 0.3)
    );
    assert_eq!(material.alpha_cutoff, Some(0.5));
    assert_eq!(material.metallic, Material::default().metallic);
    assert!(other.has::<Camera>(new_camera));
    assert_eq!(
        other.get::<Camera>(new_camera).unwrap().fov_y,
        world.get::<Camera>(camera).unwrap().fov_y
    );

    // Capturing what was spawned gives the same scene again, but for the ids.
    let again = Scene::capture(&other, &registry);
    let spawned_only: Vec<_> = again
        .entities
        .iter()
        .filter(|e| spawned.entities.iter().any(|s| s.to_bits() == e.id))
        .collect();
    assert_eq!(spawned_only.len(), 3);
    assert_eq!(
        spawned_only[0]
            .components
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["mira.Transform", "test.Health"]
    );
}

#[test]
fn what_cannot_be_restored_is_skipped_and_reported() {
    let registry = registry();
    let text = r#"{
      "version": 1,
      "entities": [
        {"id": 1, "components": {
          "mira.Transform": {"translation": [1, 2, 3]},
          "game.FromANewerVersion": {"x": 1},
          "mira.Parent": {"$entity": 999},
          "test.Health": {"current": "lots"}
        }}
      ]
    }"#;
    let scene = Scene::from_json(text).unwrap();
    let mut world = World::new();
    let spawned = scene.spawn(&mut world, &registry);
    let entity = spawned.entities[0];

    // The good component is there, with what the file left out filled in.
    let transform = world.get::<Transform>(entity).unwrap();
    assert_eq!(
        (transform.translation, transform.scale),
        (Vec3::new(1.0, 2.0, 3.0), Vec3::ONE)
    );
    assert!(
        !world.has::<Parent>(entity),
        "a reference out of the scene isn't guessed at"
    );
    assert_eq!(spawned.skipped.len(), 3, "{:?}", spawned.skipped);
    assert!(spawned.skipped[2].contains("at `current`: expected a number, found text"));

    assert!(Scene::from_json(r#"{"version": 2, "entities": []}"#).is_err());
    assert!(Scene::from_json(r#"{"entities": []}"#).is_err());
    assert!(Scene::from_json("not json").is_err());
}

#[test]
fn engine_plugins_register_their_components() {
    let mut app = App::new();
    app.add_plugins(TransformPlugin).register_type::<Health>();
    let registry = app.world.resource::<TypeRegistry>();
    let names: Vec<_> = registry.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        [
            "mira.Transform",
            "mira.Parent",
            "mira.Interpolate",
            "test.Health"
        ]
    );
}

#[test]
fn scenes_save_to_and_load_from_files_through_an_app() {
    let dir = std::env::temp_dir().join(format!("mira-scene-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("level.json");

    let mut app = App::new();
    app.add_plugins(TransformPlugin);
    let parent = app.world.spawn(Transform::from_xyz(4.0, 0.0, 0.0));
    app.world.spawn((Transform::IDENTITY, Parent(parent)));
    Scene::capture(&app.world, app.world.resource::<TypeRegistry>())
        .save(&path)
        .unwrap();

    let scene = Scene::load(&path).unwrap();
    let spawned = app
        .world
        .resource_scope(|world, registry: &mut TypeRegistry| scene.spawn(world, registry));
    assert!(spawned.skipped.is_empty());
    assert_eq!(app.world.query::<&Transform>().count(), 4);
    assert_eq!(
        app.world.get::<Parent>(spawned.entities[1]),
        Some(&Parent(spawned.entities[0]))
    );

    assert!(Scene::load(dir.join("missing.json")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_level_keeps_its_settings() {
    use crate::render::{AmbientLight, Fog};

    let mut registry = registry();
    registry.register_resource::<Fog>();
    registry.register_resource::<AmbientLight>();
    let mut world = World::new();
    world.spawn(Transform::IDENTITY);
    let fog = Fog {
        density: 0.02,
        start: 40.0,
        ..Default::default()
    };
    world.insert_resource(fog);

    // Only what the world has is saved; a scene without resources has no such key at all.
    let scene = Scene::capture(&world, &registry);
    assert_eq!(scene.resources.len(), 1);
    let text = scene.to_json();
    assert!(text.contains("mira.Fog") && !text.contains("mira.AmbientLight"));
    assert!(!Scene::capture(&World::new(), &registry)
        .to_json()
        .contains("resources"));

    let mut other = World::new();
    other.insert_resource(Fog::default());
    let mut scene = Scene::from_json(&text).unwrap();
    scene.resources.push(("game.Weather".into(), Value::Null));
    scene
        .resources
        .push(("mira.AmbientLight".into(), Value::Int(3)));
    let spawned = scene.spawn(&mut other, &registry);
    assert_eq!(other.get_resource::<Fog>(), Some(&fog));
    assert_eq!(spawned.skipped.len(), 2, "{:?}", spawned.skipped);
    assert!(!other.contains_resource::<AmbientLight>());

    // A part of the world saved as a prefab carries no settings.
    let root = world.spawn(Transform::IDENTITY);
    assert!(Scene::capture_tree(&world, &registry, root)
        .resources
        .is_empty());
}

#[test]
fn the_look_of_a_level_is_part_of_it() {
    use crate::render::{PostProcess, ShadowSettings, VolumetricLight};

    let mut registry = registry();
    registry.register_resource::<PostProcess>();
    registry.register_resource::<ShadowSettings>();
    registry.register_resource::<VolumetricLight>();
    let mut world = World::new();
    world.insert_resource(PostProcess {
        exposure: 2.5,
        taa: false,
        ..Default::default()
    });
    world.insert_resource(ShadowSettings {
        max_distance: 77.0,
        ..Default::default()
    });
    let scene = Scene::from_json(&Scene::capture(&world, &registry).to_json()).unwrap();
    assert_eq!(scene.resources.len(), 2, "only what the world has");

    let mut other = World::new();
    let spawned = scene.spawn(&mut other, &registry);
    assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
    let post = other.resource::<PostProcess>();
    assert_eq!((post.exposure, post.taa), (2.5, false));
    assert_eq!(other.resource::<ShadowSettings>().max_distance, 77.0);
    // One field by name, as a tool would set it; the rest keep their defaults.
    let exposure = Value::Map(vec![("exposure".into(), Value::Float(0.4))]);
    (registry.resource("mira.PostProcess").unwrap().insert)(&mut other, &exposure).unwrap();
    assert_eq!(other.resource::<PostProcess>().exposure, 0.4);
    assert!(
        other.resource::<PostProcess>().taa,
        "a field left out takes its default"
    );
}
