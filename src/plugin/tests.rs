//! These tests compile real plugins with the system C compiler and load them, because the
//! thing under test is the boundary itself.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use glam::Vec3;

use super::library_file_name;
use crate::{
    app::App,
    ecs::{Entity, World},
    time::TimePlugin,
    transform::{Transform, TransformPlugin},
};

const PRELUDE: &str = r#"
#include "voxl.h"
static const VoxlApi *api;
VOXL_EXPORT uint32_t voxl_plugin_abi_version(void) { return ABI; }
"#;

/// Counts frames on every entity with a transform and moves it `STEP` along X each frame.
/// Writes the number of times the plugin has been loaded into Z.
const COUNTER: &str = r#"
typedef struct { int32_t frames; } Counter;
static VoxlComponent transform_c, counter_c;
static int32_t *loads;

static void boot(VoxlSystem *s, void *user) {
    Counter c = {100};
    api->insert(s, api->spawn(s), counter_c, &c);
}

static void adopt(VoxlSystem *s, void *user) {
    VoxlEntity e;
    while (api->query_next(s, &e, NULL)) {
        Counter c = {0};
        api->insert(s, e, counter_c, &c);
    }
}

static void step(VoxlSystem *s, void *user) {
    VoxlEntity e;
    void *found[2];
    while (api->query_next(s, &e, found)) {
        VoxlTransform *t = found[0];
        Counter *c = found[1];
        t->translation[0] += STEP;
        t->translation[2] = (float)*loads;
        c->frames += 1;
    }
}

VOXL_EXPORT int32_t voxl_plugin_load(const VoxlApi *a, VoxlApp *app) {
    api = a;
    size_t size = 0, align = 0;
    transform_c = api->component_lookup(app, VOXL_STR("voxl.Transform"), &size, &align);
    if (!transform_c || size != sizeof(VoxlTransform) || align != _Alignof(VoxlTransform))
        return -2;
    counter_c = api->component_register(app, VOXL_STR("test.Counter"), sizeof(Counter),
                                        _Alignof(Counter), NULL);
    loads = api->state(app, VOXL_STR("test.loads"), sizeof(int32_t), _Alignof(int32_t));
    *loads += 1;

    VoxlTerm adopt_terms[] = {{transform_c, VOXL_WITH}, {counter_c, VOXL_WITHOUT}};
    VoxlTerm step_terms[] = {{transform_c, VOXL_WRITE}, {counter_c, VOXL_WRITE}};
    VoxlSystemDesc systems[] = {
        {VOXL_STR("boot"), VOXL_STAGE_STARTUP, 0, boot, NULL, NULL, 0},
        {VOXL_STR("adopt"), VOXL_STAGE_UPDATE, 0, adopt, NULL, adopt_terms, 2},
        {VOXL_STR("step"), VOXL_STAGE_UPDATE, 0, step, NULL, step_terms, 2},
    };
    for (int i = 0; i < 3; i++)
        if (api->system_add(app, &systems[i]) != 0) return -3;
    return 0;
}
"#;

/// Asks for the same component mutably and immutably, which the engine must refuse.
const CONFLICTING: &str = r#"
static void run(VoxlSystem *s, void *user) {}
VOXL_EXPORT int32_t voxl_plugin_load(const VoxlApi *a, VoxlApp *app) {
    api = a;
    VoxlComponent t = api->component_lookup(app, VOXL_STR("voxl.Transform"), NULL, NULL);
    VoxlTerm terms[] = {{t, VOXL_WRITE}, {t, VOXL_READ}};
    VoxlSystemDesc desc = {VOXL_STR("bad"), VOXL_STAGE_UPDATE, 0, run, NULL, terms, 2};
    return api->system_add(app, &desc);
}
"#;

/// Uses what a plugin needs to be a game by itself: input, meshes and materials, and a
/// system with two queries.
///
/// `setup` creates a mesh and spawns a visible "player". `drive` moves players right while D
/// is held and counts presses of SPACE in the player's Y. `tag` has two queries: for every
/// player it finds, it marks each other entity that has a transform (query 1) by setting Z.
const GAME: &str = r#"
typedef struct { int32_t unused; } Player;
static VoxlComponent transform_c, player_c;
static VoxlMesh *cube;

static void setup(VoxlSystem *s, void *user) {
    *cube = api->mesh_shape(s, VOXL_SHAPE_CUBE, 1.0f);
    VoxlVertex corners[3] = {{{0, 0, 0}, {0, 1, 0}, {0, 0}},
                             {{0, 0, 1}, {0, 1, 0}, {0, 1}},
                             {{1, 0, 0}, {0, 1, 0}, {1, 0}}};
    uint32_t triangle[3] = {0, 1, 2};
    VoxlMesh custom = api->mesh_create(s, corners, 3, triangle, 3);

    VoxlEntity e = api->spawn(s);
    VoxlTransform t = {{0, 0, 0}, 0, {0, 0, 0, 1}, {1, 1, 1}, 0};
    Player p = {0};
    VoxlMaterial red = {{1, 0, 0, 1}, {0, 0, 0}, 0.5f, 0.0f};
    api->insert(s, e, transform_c, &t);
    api->insert(s, e, player_c, &p);
    api->set_mesh(s, e, custom ? *cube : 0);
    api->set_material(s, e, &red);
}

static void drive(VoxlSystem *s, void *user) {
    VoxlEntity e;
    void *found[1];
    while (api->query_next(s, &e, found)) {
        VoxlTransform *t = found[0];
        if (api->key_down(s, VOXL_KEY_A + ('d' - 'a'))) t->translation[0] += 1.0f;
        if (api->key_pressed(s, VOXL_KEY_SPACE)) t->translation[1] += 1.0f;
        if (api->key_down(s, VOXL_KEY_F1 + 11) || api->mouse_down(s, VOXL_MOUSE_RIGHT))
            t->translation[1] = -100.0f;
        float motion[2];
        api->mouse_motion(s, motion);
        t->scale[0] += motion[0];
    }
}

static void tag(VoxlSystem *s, void *user) {
    VoxlEntity player, other;
    void *found[1];
    while (api->query_next(s, &player, NULL)) {
        api->query_rewind(s, 1);
        while (api->query_next_in(s, 1, &other, found))
            ((VoxlTransform *)found[0])->translation[2] += 1.0f;
        /* The player itself is not in query 1. */
        if (api->query_get_in(s, 1, player, found)) ((VoxlTransform *)found[0])->translation[2] = -1;
    }
}

VOXL_EXPORT int32_t voxl_plugin_load(const VoxlApi *a, VoxlApp *app) {
    api = a;
    transform_c = api->component_lookup(app, VOXL_STR("voxl.Transform"), NULL, NULL);
    player_c = api->component_register(app, VOXL_STR("game.Player"), sizeof(Player),
                                       _Alignof(Player), NULL);
    cube = api->state(app, VOXL_STR("game.cube"), sizeof(VoxlMesh), _Alignof(VoxlMesh));

    VoxlTerm players_moving[] = {{transform_c, VOXL_WRITE}, {player_c, VOXL_WITH}};
    VoxlTerm players[] = {{player_c, VOXL_WITH}};
    VoxlTerm others[] = {{transform_c, VOXL_WRITE}, {player_c, VOXL_WITHOUT}};
    VoxlTerm everything[] = {{transform_c, VOXL_WRITE}};
    VoxlSystemDesc systems[] = {
        {VOXL_STR("setup"), VOXL_STAGE_STARTUP, 0, setup, NULL, NULL, 0},
        {VOXL_STR("drive"), VOXL_STAGE_UPDATE, 0, drive, NULL, players_moving, 2},
        {VOXL_STR("tag"), VOXL_STAGE_UPDATE, 0, tag, NULL, players, 1},
    };
    for (int i = 0; i < 3; i++)
        if (api->system_add(app, &systems[i]) != 0) return -3;
    if (api->system_add_query(app, VOXL_STR("tag"), others, 2) != 1) return -4;
    /* A second writer of every transform would overlap `others`: must be refused. */
    if (api->system_add_query(app, VOXL_STR("tag"), everything, 1) >= 0) return -5;
    if (api->system_add_query(app, VOXL_STR("nobody"), others, 2) >= 0) return -6;
    return 0;
}
"#;

struct Workspace {
    dir: PathBuf,
}

impl Workspace {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("voxl-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn library(&self, name: &str) -> PathBuf {
        self.dir.join(library_file_name(name))
    }

    /// Compiles `body` into the shared library `name`, with the given `-D` definitions.
    fn compile(&self, name: &str, body: &str, defines: &[&str]) -> PathBuf {
        let source = self.dir.join(format!("{name}.c"));
        std::fs::write(&source, format!("{PRELUDE}{body}")).unwrap();
        let out = self.library(name);
        // Build beside the target and rename, as a real build does: the engine must never
        // see a half-written library.
        let staging = self.dir.join(format!("{name}.tmp"));
        let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
        let output = Command::new("cc")
            .args([
                "-shared",
                "-fPIC",
                "-std=c11",
                "-Wall",
                "-Werror",
                "-Wno-unused-parameter",
            ])
            .arg("-I")
            .arg(include)
            .args(defines.iter().map(|d| format!("-D{d}")))
            .arg("-o")
            .arg(&staging)
            .arg(&source)
            .output()
            .expect("these tests need a C compiler (`cc`) on the PATH");
        assert!(
            output.status.success(),
            "cc failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::rename(&staging, &out).unwrap();
        out
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
    app
}

fn counter(world: &World, entity: Entity) -> Option<i32> {
    let id = world.named_component_id("test.Counter")?;
    let storage = world.erased_storage(world.named_component(id)?.key)?;
    // SAFETY: nothing else is using the world, and the component is one `int32_t`.
    unsafe { Some(*storage.value_ptr(entity)?.cast::<i32>()) }
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world.get::<Transform>(entity).unwrap().translation
}

#[test]
fn a_c_plugin_runs_reloads_and_keeps_its_data() {
    let workspace = Workspace::new("reload");
    let library = workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=1.0f"]);

    let mut app = app();
    let moved = app.world.spawn(Transform::IDENTITY);
    let other = app.world.spawn(Transform::from_xyz(0.0, 5.0, 0.0));
    let bystander = app.world.spawn_empty();
    app.update(); // the app is running before the plugin arrives

    app.load_native_plugin(&library).unwrap();
    assert_eq!(
        app.world.entity_count(),
        4,
        "the startup system spawned one entity, late"
    );
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(position(&app, moved), Vec3::new(3.0, 0.0, 1.0));
    assert_eq!(position(&app, other), Vec3::new(3.0, 5.0, 1.0));
    assert_eq!(counter(&app.world, moved), Some(3));
    assert_eq!(counter(&app.world, bystander), None);

    // Nothing changed on disk, so nothing reloads.
    assert_eq!(app.reload_native_plugins(), 0);

    // Rebuild with different behaviour, while the app is running.
    workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=10.0f"]);
    assert_eq!(app.reload_native_plugins(), 1);
    for _ in 0..2 {
        app.update();
    }
    // New code (steps of 10), old data (it carried on from x = 3 and frame 3), and state that
    // remembers this is the second load.
    assert_eq!(position(&app, moved), Vec3::new(23.0, 0.0, 2.0));
    assert_eq!(counter(&app.world, moved), Some(5));
    assert_eq!(
        app.world.entity_count(),
        4,
        "startup systems don't run again on reload"
    );
    assert_eq!(
        app.native_plugins().loaded().collect::<Vec<_>>(),
        [("counter", 2)]
    );
}

#[test]
fn the_frame_loop_reloads_a_changed_plugin_once_it_settles() {
    let workspace = Workspace::new("watch");
    let library = workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=1.0f"]);
    let mut app = app();
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();
    app.native_plugins().check_interval = std::time::Duration::ZERO;
    app.update();
    assert_eq!(position(&app, entity).x, 1.0);

    workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=100.0f"]);
    app.update(); // sees the change, waits to see if the file is still being written
    assert_eq!(position(&app, entity).x, 2.0);
    app.update(); // unchanged since last look: reloads, then runs the new code
    assert_eq!(position(&app, entity).x, 102.0);
}

#[test]
fn a_broken_rebuild_leaves_the_old_version_running() {
    let workspace = Workspace::new("broken");
    let library = workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=1.0f"]);
    let mut app = app();
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();
    app.update();

    std::fs::write(&library, b"this is not a shared library").unwrap();
    assert_eq!(app.reload_native_plugins(), 0);
    app.update();
    assert_eq!(
        position(&app, entity).x,
        2.0,
        "the old code is still stepping"
    );

    // A good build afterwards is picked up.
    workspace.compile("counter", COUNTER, &["ABI=VOXL_ABI_VERSION", "STEP=5.0f"]);
    assert_eq!(app.reload_native_plugins(), 1);
    app.update();
    assert_eq!(position(&app, entity).x, 7.0);
}

#[test]
fn unusable_plugins_are_refused() {
    let workspace = Workspace::new("refused");
    let mut app = app();

    let missing = app.load_native_plugin(workspace.library("nowhere"));
    assert!(missing.is_err());

    let old = workspace.compile("old", COUNTER, &["ABI=999", "STEP=1.0f"]);
    let err = app.load_native_plugin(&old).err().unwrap();
    assert!(format!("{err:#}").contains("version 999"), "{err:#}");

    let conflicting = workspace.compile("conflicting", CONFLICTING, &["ABI=VOXL_ABI_VERSION"]);
    assert!(app.load_native_plugin(&conflicting).is_err());

    // None of that left anything behind.
    app.world.spawn(Transform::IDENTITY);
    app.update();
    assert_eq!(app.native_plugins().loaded().count(), 0);
}

/// The Rust bindings, through the real example plugin.
#[test]
fn the_rust_example_plugin_loads() {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Cell {
        x: f32,
        z: f32,
    }
    impl crate::ecs::Component for Cell {}

    // Built into its own directory: the outer `cargo test` may hold the lock on the main one.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target").join("plugin-tests");
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "wave", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success(), "building the wave plugin failed");
    let library = target.join("debug").join(library_file_name("wave"));

    let mut app = app();
    app.world.export_component::<Cell>("demo.Cell");
    let entity = app
        .world
        .spawn((Transform::IDENTITY, Cell { x: 1.0, z: 2.0 }));
    app.load_native_plugin(library).unwrap();
    app.world
        .resource_mut::<crate::time::Time>()
        .advance_by(std::time::Duration::from_millis(250));
    app.update();
    app.update();

    let rider = app.world.named_component_id("wave.Rider").unwrap();
    let key = app.world.named_component(rider).unwrap().key;
    assert!(app.world.erased_storage(key).unwrap().contains(entity));
    let y = position(&app, entity).y;
    assert!(y != 0.0 && y.abs() <= 1.0, "the wave moved it to y = {y}");
}

#[test]
fn a_plugin_can_read_input_draw_things_and_run_two_queries() {
    use crate::{
        assets::Assets,
        input::{ButtonInput, InputPlugin, KeyCode, Mouse, MouseButton},
        render::{Material, Mesh, Mesh3d},
    };

    let workspace = Workspace::new("game");
    let library = workspace.compile("game", GAME, &["ABI=VOXL_ABI_VERSION"]);
    let mut app = app();
    app.add_plugins(InputPlugin).init_resource::<Assets<Mesh>>();
    let rock = app.world.spawn(Transform::from_xyz(5.0, 0.0, 0.0));
    app.load_native_plugin(&library).unwrap();
    app.update();

    // The startup system made two meshes and one visible entity.
    assert_eq!(app.world.resource::<Assets<Mesh>>().len(), 2);
    let players = app.world.query::<(Entity, &Mesh3d, &Material)>();
    let (player, _, material) = players.single();
    assert_eq!(material.color.to_array(), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(material.roughness, 0.5);
    // Nothing is pressed yet; the two-query system marked the rock, and only the rock.
    assert_eq!(position(&app, player), Vec3::ZERO);
    assert_eq!(position(&app, rock), Vec3::new(5.0, 0.0, 1.0));

    let keys = app.world.resource_mut::<ButtonInput<KeyCode>>();
    keys.press(KeyCode::KeyD);
    keys.press(KeyCode::Space);
    app.world.resource_mut::<Mouse>().delta = glam::Vec2::new(0.25, 9.0);
    app.update();
    app.update();
    // D is held for both frames; SPACE only counts on the frame it went down.
    assert_eq!(position(&app, player), Vec3::new(2.0, 1.0, 0.0));
    assert_eq!(app.world.get::<Transform>(player).unwrap().scale.x, 1.25);
    assert_eq!(position(&app, rock).z, 3.0);

    app.world
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyD);
    app.world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Right);
    app.update();
    assert_eq!(position(&app, player), Vec3::new(2.0, -100.0, 0.0));
}

/// A Haskell plugin: moves every entity with a transform `STEP` along X each frame and counts
/// the frames in a component of its own. Each run also allocates and forces a major garbage
/// collection, so that a collector still holding on to a replaced version would be caught.
const HASKELL: &str = r#"
{-# LANGUAGE ForeignFunctionInterface #-}
module Stepper where

import Control.Concurrent (rtsSupportsBoundThreads)
import Control.Monad (unless)
import Data.Int (Int32)
import Foreign (Ptr)
import Foreign.C.Types (CInt (..))
import System.Mem (performMajorGC)
import Voxl

foreign import ccall unsafe "voxl_hs_nonmoving_gc" nonmovingGC :: IO CInt

foreign export ccall voxl_hs_main :: Ptr () -> IO CInt

voxl_hs_main :: Ptr () -> IO CInt
voxl_hs_main = plugin $ \app -> do
  -- The old generation must be collected without long pauses: the non-moving collector,
  -- marking concurrently, which needs the threaded runtime.
  nonmoving <- nonmovingGC
  unless (nonmoving /= 0) $ ioError (userError "the non-moving collector is off")
  unless rtsSupportsBoundThreads $ ioError (userError "not the threaded runtime")
  transform <- lookupComponent app "voxl.Transform"
  frames <- registerComponent app "hs.Frames" :: IO (Component Int32)
  addSystem app "adopt" Update (with transform <* without frames) $ \sys entity () ->
    insert sys entity frames 0
  addSystem app "step" Update ((,) <$> write transform <*> write frames) $ \_ _ (place, count) -> do
    modifyRef place $ \t -> t {translation = translation t + V3 STEP 0 0}
    modifyRef count (+ 1)
  addSystem_ app "churn" Update $ \_ -> do
    let total = sum (map fromIntegral [1 .. 20000 :: Int]) :: Double
    total `seq` performMajorGC
"#;

/// Whether the Haskell tests can run. Without GHC they are skipped, loudly; where the suite
/// must be complete (CI sets `VOXL_REQUIRE_GHC=1`) a missing GHC fails them instead.
fn have_ghc() -> bool {
    if Command::new("ghc").arg("--version").output().is_ok() {
        return true;
    }
    assert!(
        std::env::var("VOXL_REQUIRE_GHC").as_deref() != Ok("1"),
        "GHC is required (VOXL_REQUIRE_GHC=1) but not installed"
    );
    eprintln!("skipped: GHC is not installed, so the Haskell plugins were not tested");
    false
}

fn build_haskell(workspace: &Workspace, step: &str) -> PathBuf {
    let source = workspace.dir.join("Stepper.hs");
    std::fs::write(&source, HASKELL.replace("STEP", step)).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings/haskell/build-plugin.sh");
    let output = Command::new(script)
        .arg("stepper")
        .arg(&source)
        .arg(&workspace.dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "building the Haskell plugin failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    workspace.library("stepper")
}

#[test]
fn a_haskell_plugin_runs_and_reloads_repeatedly() {
    if !have_ghc() {
        return;
    }
    let workspace = Workspace::new("haskell");
    let library = build_haskell(&workspace, "1");

    let mut app = app();
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(position(&app, entity).x, 5.0);

    // Reload several times, running frames (and collections) on each new version while the
    // old ones are still mapped but no longer called.
    let mut expected = 5.0;
    for (round, step) in [10.0, 100.0, 1000.0].into_iter().enumerate() {
        build_haskell(&workspace, &format!("{step}"));
        assert_eq!(app.reload_native_plugins(), 1);
        for _ in 0..20 {
            app.update();
        }
        expected += 20.0 * step;
        assert_eq!(
            position(&app, entity).x,
            expected,
            "after reload {}",
            round + 1
        );
    }

    let frames = app.world.named_component_id("hs.Frames").unwrap();
    let key = app.world.named_component(frames).unwrap().key;
    // SAFETY: nothing else is using the world, and the component is one 32-bit integer.
    let count = unsafe {
        *app.world
            .erased_storage(key)
            .unwrap()
            .value_ptr(entity)
            .unwrap()
            .cast::<i32>()
    };
    assert_eq!(count, 65, "the frame count carried across every reload");
    assert_eq!(
        app.native_plugins().loaded().collect::<Vec<_>>(),
        [("stepper", 4)]
    );
}

/// The example game, which is written entirely as a Haskell plugin: the whole widened
/// interface (drawing, input, two queries, spawning and despawning) through the bindings a
/// plugin author would use.
#[test]
fn the_haskell_game_plays() {
    use crate::{
        assets::Assets,
        input::{ButtonInput, InputPlugin, KeyCode},
        render::{Material, Mesh, Mesh3d},
        time::Time,
    };
    use std::time::Duration;

    if !have_ghc() {
        return;
    }
    let workspace = Workspace::new("chase");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(root.join("bindings/haskell/build-plugin.sh"))
        .arg("chase")
        .arg(root.join("plugins/chase/Chase.hs"))
        .arg(&workspace.dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "building the game failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut app = app();
    app.add_plugins(InputPlugin).init_resource::<Assets<Mesh>>();
    app.load_native_plugin(workspace.library("chase")).unwrap();
    // A fixed tenth of a second per frame, so the test doesn't depend on how fast it runs.
    app.world
        .resource_mut::<Time>()
        .set_fixed_step(Some(Duration::from_millis(100)));
    let frame = |app: &mut App| app.update();
    frame(&mut app);
    frame(&mut app);

    let drawn = |app: &mut App| app.world.query::<(&Mesh3d, &Material)>().count();
    assert_eq!(drawn(&mut app), 12, "the field, the player and ten pickups");
    let ids = |app: &App, name: &str| {
        let id = app.world.named_component_id(name).unwrap();
        let key = app.world.named_component(id).unwrap().key;
        app.world.erased_storage(key).unwrap().entities().to_vec()
    };
    let player = ids(&app, "chase.Player")[0];
    let start = position(&app, player);

    // Steering.
    app.world
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyD);
    frame(&mut app);
    frame(&mut app);
    app.world
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyD);
    let moved = position(&app, player);
    assert!(
        moved.x > start.x + 0.5 && moved.z == start.z,
        "{start} -> {moved}"
    );

    // Collecting: put the player on a pickup. It goes, and another appears elsewhere.
    let pickups = ids(&app, "chase.Pickup");
    let target = pickups[3];
    let spot = position(&app, target);
    app.world.get_mut::<Transform>(player).unwrap().translation = Vec3::new(spot.x, 0.5, spot.z);
    frame(&mut app);
    frame(&mut app);
    assert!(
        !app.world.contains_entity(target),
        "the pickup was collected"
    );
    assert_eq!(
        ids(&app, "chase.Pickup").len(),
        pickups.len(),
        "and replaced"
    );
    assert_eq!(drawn(&mut app), 12);
}
