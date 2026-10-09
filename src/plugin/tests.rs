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
#include "mira.h"
static const MiraApi *api;
MIRA_EXPORT uint32_t mira_plugin_abi_version(void) { return ABI; }
"#;

/// Counts frames on every entity with a transform and moves it `STEP` along X each frame.
/// Writes the number of times the plugin has been loaded into Z.
const COUNTER: &str = r#"
typedef struct { int32_t frames; } Counter;
static MiraComponent transform_c, counter_c;
static int32_t *loads;

static void boot(MiraSystem *s, void *user) {
    Counter c = {100};
    api->insert(s, api->spawn(s), counter_c, &c);
}

static void adopt(MiraSystem *s, void *user) {
    MiraEntity e;
    while (api->query_next(s, &e, NULL)) {
        Counter c = {0};
        api->insert(s, e, counter_c, &c);
    }
}

static void step(MiraSystem *s, void *user) {
    MiraEntity e;
    void *found[2];
    while (api->query_next(s, &e, found)) {
        MiraTransform *t = found[0];
        Counter *c = found[1];
#ifdef LIMIT
        if (t->translation[0] >= LIMIT) {
            api->system_fail(s, MIRA_STR("ran off the edge"), MIRA_STR("counter.c: step"));
            api->spawn(s); /* dropped with the rest of this run */
            return;
        }
#endif
        t->translation[0] += STEP;
        t->translation[2] = (float)*loads;
        c->frames += 1;
    }
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    size_t size = 0, align = 0;
    transform_c = api->component_lookup(app, MIRA_STR("mira.Transform"), &size, &align);
    if (!transform_c || size != sizeof(MiraTransform) || align != _Alignof(MiraTransform))
        return -2;
    counter_c = api->component_register(app, MIRA_STR("test.Counter"), sizeof(Counter),
                                        _Alignof(Counter), NULL);
    loads = api->state(app, MIRA_STR("test.loads"), sizeof(int32_t), _Alignof(int32_t));
    *loads += 1;

    MiraTerm adopt_terms[] = {{transform_c, MIRA_WITH}, {counter_c, MIRA_WITHOUT}};
    MiraTerm step_terms[] = {{transform_c, MIRA_WRITE}, {counter_c, MIRA_WRITE}};
    MiraSystemDesc systems[] = {
        {MIRA_STR("boot"), MIRA_STAGE_STARTUP, 0, boot, NULL, NULL, 0},
        {MIRA_STR("adopt"), MIRA_STAGE_UPDATE, 0, adopt, NULL, adopt_terms, 2},
        {MIRA_STR("step"), MIRA_STAGE_UPDATE, 0, step, NULL, step_terms, 2},
    };
    for (int i = 0; i < 3; i++)
        if (api->system_add(app, &systems[i]) != 0) return -3;
    return 0;
}
"#;

/// Asks for the same component mutably and immutably, which the engine must refuse.
const CONFLICTING: &str = r#"
static void run(MiraSystem *s, void *user) {}
MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    MiraComponent t = api->component_lookup(app, MIRA_STR("mira.Transform"), NULL, NULL);
    MiraTerm terms[] = {{t, MIRA_WRITE}, {t, MIRA_READ}};
    MiraSystemDesc desc = {MIRA_STR("bad"), MIRA_STAGE_UPDATE, 0, run, NULL, terms, 2};
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
static MiraComponent transform_c, player_c;
static MiraMesh *cube;

static void setup(MiraSystem *s, void *user) {
    *cube = api->mesh_shape(s, MIRA_SHAPE_CUBE, 1.0f);
    MiraVertex corners[3] = {{{0, 0, 0}, {0, 1, 0}, {0, 0}},
                             {{0, 0, 1}, {0, 1, 0}, {0, 1}},
                             {{1, 0, 0}, {0, 1, 0}, {1, 0}}};
    uint32_t triangle[3] = {0, 1, 2};
    MiraMesh custom = api->mesh_create(s, corners, 3, triangle, 3);

    MiraEntity e = api->spawn(s);
    MiraTransform t = {{0, 0, 0}, 0, {0, 0, 0, 1}, {1, 1, 1}, 0};
    Player p = {0};
    MiraMaterial red = {{1, 0, 0, 1}, {0, 0, 0}, 0.5f, 0.0f};
    api->insert(s, e, transform_c, &t);
    api->insert(s, e, player_c, &p);
    api->set_mesh(s, e, custom ? *cube : 0);
    api->set_material(s, e, &red);
}

static void drive(MiraSystem *s, void *user) {
    MiraEntity e;
    void *found[1];
    while (api->query_next(s, &e, found)) {
        MiraTransform *t = found[0];
        if (api->key_down(s, MIRA_KEY_A + ('d' - 'a'))) t->translation[0] += 1.0f;
        if (api->key_pressed(s, MIRA_KEY_SPACE)) t->translation[1] += 1.0f;
        if (api->key_down(s, MIRA_KEY_F1 + 11) || api->mouse_down(s, MIRA_MOUSE_RIGHT))
            t->translation[1] = -100.0f;
        float motion[2];
        api->mouse_motion(s, motion);
        t->scale[0] += motion[0];
    }
}

static void tag(MiraSystem *s, void *user) {
    MiraEntity player, other;
    void *found[1];
    while (api->query_next(s, &player, NULL)) {
        api->query_rewind(s, 1);
        while (api->query_next_in(s, 1, &other, found))
            ((MiraTransform *)found[0])->translation[2] += 1.0f;
        /* The player itself is not in query 1. */
        if (api->query_get_in(s, 1, player, found)) ((MiraTransform *)found[0])->translation[2] = -1;
    }
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    transform_c = api->component_lookup(app, MIRA_STR("mira.Transform"), NULL, NULL);
    player_c = api->component_register(app, MIRA_STR("game.Player"), sizeof(Player),
                                       _Alignof(Player), NULL);
    cube = api->state(app, MIRA_STR("game.cube"), sizeof(MiraMesh), _Alignof(MiraMesh));

    MiraTerm players_moving[] = {{transform_c, MIRA_WRITE}, {player_c, MIRA_WITH}};
    MiraTerm players[] = {{player_c, MIRA_WITH}};
    MiraTerm others[] = {{transform_c, MIRA_WRITE}, {player_c, MIRA_WITHOUT}};
    MiraTerm everything[] = {{transform_c, MIRA_WRITE}};
    MiraSystemDesc systems[] = {
        {MIRA_STR("setup"), MIRA_STAGE_STARTUP, 0, setup, NULL, NULL, 0},
        {MIRA_STR("drive"), MIRA_STAGE_UPDATE, 0, drive, NULL, players_moving, 2},
        {MIRA_STR("tag"), MIRA_STAGE_UPDATE, 0, tag, NULL, players, 1},
    };
    for (int i = 0; i < 3; i++)
        if (api->system_add(app, &systems[i]) != 0) return -3;
    if (api->system_add_query(app, MIRA_STR("tag"), others, 2) != 1) return -4;
    /* A second writer of every transform would overlap `others`: must be refused. */
    if (api->system_add_query(app, MIRA_STR("tag"), everything, 1) >= 0) return -5;
    if (api->system_add_query(app, MIRA_STR("nobody"), others, 2) >= 0) return -6;
    return 0;
}
"#;

/// Owns a scene (camera, sun, ambient light), drops a ball onto the ground with physics, and
/// sends a `test.Tick` event every frame. What it observes goes in a `test.Report` component.
const SCENE: &str = r#"
typedef struct { int32_t n; } Tick;
typedef struct { int32_t contacts; float speed; float ray; int32_t ticks_sent; } Report;
static MiraComponent transform_c, report_c;
static MiraEvent tick_e, contact_e;
static MiraEntity *ball;

static MiraTransform at(float x, float y, float z) {
    MiraTransform t = {{x, y, z}, 0, {0, 0, 0, 1}, {1, 1, 1}, 0};
    return t;
}

static void setup(MiraSystem *s, void *user) {
    MiraTransform origin = at(0, 0, 0), above = at(0, 5, 0);

    MiraEntity camera = api->spawn(s);
    MiraCamera lens = {1.0f, 0.5f, 1};
    api->insert(s, camera, transform_c, &origin);
    api->set_camera(s, camera, &lens);

    MiraEntity sun = api->spawn(s);
    MiraLight light = {{1.0f, 0.5f, 0.25f}, 4.0f, 0};
    api->insert(s, sun, transform_c, &origin);
    api->set_light(s, sun, &light);

    float sky[3] = {0.1f, 0.2f, 0.3f};
    api->set_ambient(s, sky, 0.7f);
    api->set_window_title(s, MIRA_STR("no window here; must not crash"));

    MiraEntity ground = api->spawn(s);
    MiraCollider floor = {MIRA_COLLIDER_GROUND, {0, 0, 0}, 0.5f, 0.0f, 0};
    api->insert(s, ground, transform_c, &origin);
    api->set_collider(s, ground, &floor);

    *ball = api->spawn(s);
    MiraCollider round = {MIRA_COLLIDER_SPHERE, {0.5f, 0, 0}, 0.5f, 0.0f, 0};
    MiraBody body = {MIRA_BODY_DYNAMIC, 2.0f, {0, 0, 0}, 0};
    Report report = {0, 0, 0, 0};
    api->insert(s, *ball, transform_c, &above);
    api->insert(s, *ball, report_c, &report);
    api->set_collider(s, *ball, &round);
    api->set_body(s, *ball, &body);
}

static void observe(MiraSystem *s, void *user) {
    MiraEntity e;
    void *found[1];
    while (api->query_next(s, &e, found)) {
        Report *report = found[0];

        Tick tick = {report->ticks_sent++};
        api->event_send(s, tick_e, &tick);

        MiraContact contact;
        while (api->event_next(s, contact_e, &contact))
            if (contact.a == *ball || contact.b == *ball) report->contacts += 1;

        float v[3];
        report->speed = api->velocity(s, e, v) ? (v[1] < 0 ? -v[1] : v[1]) : -1.0f;

        float from[3] = {0, 10, 0}, down[3] = {0, -3, 0};
        MiraRayHit hit;
        report->ray = api->raycast(s, from, down, 100.0f, &hit) && hit.entity == *ball
                          ? hit.distance : -1.0f;
        if (api->key_pressed(s, MIRA_KEY_ENTER)) {
            float up[3] = {0, 20, 0};
            api->apply_impulse(s, e, up);
        }
    }
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    transform_c = api->component_lookup(app, MIRA_STR("mira.Transform"), NULL, NULL);
    report_c = api->component_register(app, MIRA_STR("test.Report"), sizeof(Report),
                                       _Alignof(Report), NULL);
    ball = api->state(app, MIRA_STR("test.ball"), sizeof(MiraEntity), _Alignof(MiraEntity));
    tick_e = api->event_register(app, MIRA_STR("test.Tick"), sizeof(Tick));
    contact_e = api->event_register(app, MIRA_STR("mira.Contact"), sizeof(MiraContact));
    if (!tick_e || !contact_e) return -2;
    /* The same name with another size would make the two sides misread each other. */
    if (api->event_register(app, MIRA_STR("test.Tick"), 64)) return -3;

    MiraTerm reports[] = {{report_c, MIRA_WRITE}};
    MiraSystemDesc systems[] = {
        {MIRA_STR("setup"), MIRA_STAGE_STARTUP, 0, setup, NULL, NULL, 0},
        {MIRA_STR("observe"), MIRA_STAGE_UPDATE, 0, observe, NULL, reports, 1},
    };
    for (int i = 0; i < 2; i++)
        if (api->system_add(app, &systems[i]) != 0) return -4;
    return 0;
}
"#;

/// A separate plugin that knows nothing of the first but the name and size of its event.
const LISTENER: &str = r#"
typedef struct { int32_t n; } Tick;
typedef struct { int32_t count; int32_t sum; } Heard;
static MiraComponent heard_c;
static MiraEvent tick_e;

static void setup(MiraSystem *s, void *user) {
    Heard heard = {0, 0};
    api->insert(s, api->spawn(s), heard_c, &heard);
}

static void listen(MiraSystem *s, void *user) {
    MiraEntity e;
    void *found[1];
    while (api->query_next(s, &e, found)) {
        Heard *heard = found[0];
        Tick tick;
        while (api->event_next(s, tick_e, &tick)) {
            heard->count += 1;
            heard->sum += tick.n;
        }
    }
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    heard_c = api->component_register(app, MIRA_STR("test.Heard"), sizeof(Heard),
                                      _Alignof(Heard), NULL);
    tick_e = api->event_register(app, MIRA_STR("test.Tick"), sizeof(Tick));
    MiraTerm heard[] = {{heard_c, MIRA_WRITE}};
    MiraSystemDesc systems[] = {
        {MIRA_STR("setup"), MIRA_STAGE_STARTUP, 0, setup, NULL, NULL, 0},
        {MIRA_STR("listen"), MIRA_STAGE_UPDATE, 0, listen, NULL, heard, 1},
    };
    for (int i = 0; i < 2; i++)
        if (api->system_add(app, &systems[i]) != 0) return -4;
    return tick_e ? 0 : -2;
}
"#;

/// A plugin whose component the engine can understand, because the plugin describes it; and
/// which loads an image and a model from files.
const INVENTORY: &str = r#"
#include <stddef.h>
typedef struct { float weight; int32_t count; uint8_t rare; MiraEntity owner; float tint[3]; } Item;
static MiraComponent transform_c, item_c;
static MiraEntity *hen, *camp;

static MiraTransform at(float x, float y, float z) {
    MiraTransform t = {{x, y, z}, 0, {0, 0, 0, 1}, {1, 1, 1}, 0};
    return t;
}

static void setup(MiraSystem *s, void *user) {
    MiraTransform here = at(0, 0, 0), there = at(5, 0, 0);
    MiraEntity owner = api->spawn(s);
    api->insert(s, owner, transform_c, &here);

    MiraEntity sword = api->spawn(s);
    Item item = {2.5f, 3, 1, owner, {0.1f, 0.2f, 0.3f}};
    api->insert(s, sword, transform_c, &here);
    api->insert(s, sword, item_c, &item);
    api->set_mesh(s, sword, api->mesh_shape(s, MIRA_SHAPE_CUBE, 1.0f));
    MiraImage tile = api->image_load(s, MIRA_STR("tile.png"));
    if (tile != api->image_load(s, MIRA_STR("tile.png"))) tile = 0; /* one name, one image */
    api->set_textures(s, sword, tile, 0, 0);

    *hen = api->spawn_model(s, MIRA_STR("hen.glb"), &there);
    api->spawn_model(s, MIRA_STR("no-such-model.glb"), &there); /* logged, not fatal */
    *camp = api->spawn_prefab(s, MIRA_STR("camp.json"), &there);

    /* A lantern carried by the owner: its place is relative to the owner's. */
    MiraTransform beside = at(0, 2, 0);
    MiraEntity lantern = api->spawn(s);
    api->insert(s, lantern, transform_c, &beside);
    api->set_parent(s, lantern, owner);
}

static void clear(MiraSystem *s, void *user) {
    if (api->key_pressed(s, MIRA_KEY_BACKSPACE)) api->despawn_tree(s, *hen);
}

/* The item's place shows its total weight, so a change to the component from outside (an
 * inspector, a loaded scene) is something the plugin acts on. */
static void weigh(MiraSystem *s, void *user) {
    MiraEntity e;
    void *found[2];
    while (api->query_next(s, &e, found)) {
        MiraTransform *t = found[0];
        const Item *item = found[1];
        t->translation[0] = item->weight * (float)item->count;
    }
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    transform_c = api->component_lookup(app, MIRA_STR("mira.Transform"), NULL, NULL);
    item_c = api->component_register(app, MIRA_STR("inv.Item"), sizeof(Item), _Alignof(Item), NULL);

    MiraField fields[] = {
        {MIRA_STR("weight"), MIRA_FIELD_F32, 1, offsetof(Item, weight)},
        {MIRA_STR("count"), MIRA_FIELD_I32, 1, offsetof(Item, count)},
        {MIRA_STR("rare"), MIRA_FIELD_BOOL, 1, offsetof(Item, rare)},
        {MIRA_STR("owner"), MIRA_FIELD_ENTITY, 1, offsetof(Item, owner)},
        {MIRA_STR("tint"), MIRA_FIELD_F32, 3, offsetof(Item, tint)},
    };
    /* A field that runs off the end of the component, and describing an engine component,
     * must both be refused. */
    MiraField too_far[] = {{MIRA_STR("x"), MIRA_FIELD_F64, 1, sizeof(Item) - 4}};
    if (api->component_describe(app, item_c, too_far, 1) == 0) return -2;
    if (api->component_describe(app, transform_c, fields, 5) == 0) return -3;
    if (api->component_describe(app, item_c, fields, 5) != 0) return -4;

    MiraTerm items[] = {{transform_c, MIRA_WRITE}, {item_c, MIRA_READ}};
    hen = api->state(app, MIRA_STR("inv.hen"), sizeof(MiraEntity), _Alignof(MiraEntity));
    camp = api->state(app, MIRA_STR("inv.camp"), sizeof(MiraEntity), _Alignof(MiraEntity));
    MiraSystemDesc systems[] = {
        {MIRA_STR("setup"), MIRA_STAGE_STARTUP, 0, setup, NULL, NULL, 0},
        {MIRA_STR("weigh"), MIRA_STAGE_UPDATE, 0, weigh, NULL, items, 2},
        {MIRA_STR("clear"), MIRA_STAGE_UPDATE, 0, clear, NULL, NULL, 0},
    };
    for (int i = 0; i < 3; i++)
        if (api->system_add(app, &systems[i]) != 0) return -5;
    return 0;
}
"#;

struct Workspace {
    dir: PathBuf,
}

impl Workspace {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mira-test-{}-{test}", std::process::id()));
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
    let library = workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=1.0f"]);

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
    workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=10.0f"]);
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
fn a_plugin_system_that_fails_pauses_the_game_until_it_is_fixed() {
    use crate::live::Live;

    let workspace = Workspace::new("fragile");
    let defines = ["ABI=MIRA_ABI_VERSION", "STEP=1.0f", "LIMIT=3.0f"];
    let library = workspace.compile("counter", COUNTER, &defines);
    let mut app = app();
    app.world.resource_mut::<Live>().catch_failures = true;
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(position(&app, entity).x, 3.0);
    assert!(!app.world.resource::<Live>().is_paused());
    let entities = app.world.entity_count();

    // The frame it fails on, and some more: the game holds still and stays alive.
    for _ in 0..4 {
        app.update();
    }
    let live = app.world.resource::<Live>();
    assert!(live.is_paused());
    let [failure] = live.failures() else {
        panic!("one failure, not {:?}", live.failures());
    };
    assert_eq!(failure.system, "counter::step");
    assert_eq!(failure.message, "ran off the edge");
    assert_eq!(failure.stack, "counter.c: step");
    assert_eq!(position(&app, entity).x, 3.0);
    assert_eq!(app.world.entity_count(), entities, "what the failed run queued was dropped");

    // Fix the code and save. The plugin reloads, and the game carries on by itself.
    workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=10.0f"]);
    assert_eq!(app.reload_native_plugins(), 1);
    app.update();
    assert!(!app.world.resource::<Live>().is_paused());
    assert_eq!(position(&app, entity).x, 13.0);

    // Where failures aren't caught (a release build), it is logged and the game goes on.
    app.world.resource_mut::<Live>().catch_failures = false;
    workspace.compile("counter", COUNTER, &defines);
    assert_eq!(app.reload_native_plugins(), 1);
    app.update();
    app.update();
    assert_eq!(position(&app, entity).x, 13.0);
    assert!(!app.world.resource::<Live>().is_paused());
}

/// A plugin that tells the signal graph facts, builds a rule on them and on the host's
/// signals, and acts on a signal.
const RULES: &str = r#"
static MiraComponent transform_c;
static int32_t *frames;

static void facts(MiraSystem *s, void *user) {
    *frames += 1;
    api->signal_set(s, MIRA_STR("c.frames"), (double)*frames, 1);
    api->signal_set(s, MIRA_STR("c.even"), *frames % 2 == 0, 0);
    api->signal_define(s, MIRA_STR("c.odd"), MIRA_SIGNAL_NOT, 0, MIRA_STR("c.even"));
    api->signal_define(s, MIRA_STR("c.late"), MIRA_SIGNAL_GREATER_OR_EQUAL, 0,
                       MIRA_STR("c.frames host.limit"));
    api->signal_define(s, MIRA_STR("c.go"), MIRA_SIGNAL_AND, 0, MIRA_STR("c.late  host.allowed"));
    api->signal_define(s, MIRA_STR("c.bad"), 99, 0, MIRA_STR("c.even")); /* logged, not fatal */
}

static void act(MiraSystem *s, void *user) {
    double go = 0, nothing = 5;
    if (api->signal_get(s, MIRA_STR("c.nothing"), &nothing) != 0 || nothing != 5) return;
    if (!api->signal_get(s, MIRA_STR("c.go"), &go) || go == 0) return;
    MiraEntity e;
    void *found[1];
    while (api->query_next(s, &e, found)) ((MiraTransform *)found[0])->translation[0] += 1;
}

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *a, MiraApp *app) {
    api = a;
    transform_c = api->component_lookup(app, MIRA_STR("mira.Transform"), NULL, NULL);
    frames = api->state(app, MIRA_STR("rules.frames"), sizeof(int32_t), _Alignof(int32_t));
    MiraTerm moved[] = {{transform_c, MIRA_WRITE}};
    MiraSystemDesc systems[] = {
        {MIRA_STR("facts"), MIRA_STAGE_UPDATE, 0, facts, NULL, NULL, 0},
        {MIRA_STR("act"), MIRA_STAGE_UPDATE, 0, act, NULL, moved, 1},
    };
    for (int i = 0; i < 2; i++)
        if (api->system_add(app, &systems[i]) != 0) return -3;
    return transform_c ? 0 : -2;
}
"#;

#[test]
fn a_plugin_can_set_define_and_read_signals() {
    use crate::signal::{Signal, Signals};

    let workspace = Workspace::new("rules");
    let library = workspace.compile("rules", RULES, &["ABI=MIRA_ABI_VERSION"]);
    let mut app = app();
    app.world.resource_mut::<Signals>().set("host.limit", 4.0);
    app.world.resource_mut::<Signals>().set("host.allowed", true);
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();

    // Frame n: the plugin sets its facts; frame n + 1: the graph has them; so the rule
    // `c.go` (frames >= 4, and allowed) is first true on frame 5, and `act` moves from then.
    for _ in 0..4 {
        app.update();
    }
    let signals = app.world.resource::<Signals>();
    assert_eq!(signals.get("c.frames"), Some(Signal::Number(3.0)), "as of the last update");
    assert!(signals.is_true("c.odd") && !signals.is_true("c.late"));
    assert!(signals.get("c.bad").is_none());
    assert_eq!(position(&app, entity).x, 0.0);
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world.resource::<Signals>().is_true("c.go"));
    assert_eq!(position(&app, entity).x, 3.0);

    // The host changes its mind, and the plugin's rule follows at once.
    app.world.resource_mut::<Signals>().set("host.allowed", false);
    app.update();
    app.update();
    assert_eq!(position(&app, entity).x, 3.0);
    // The plugin's rules are data like any other: seen in the graph, and rewirable.
    let graph = app.world.resource::<Signals>().graph();
    let go = graph.iter().find(|node| node.name == "c.go").unwrap();
    assert_eq!((go.kind.as_str(), go.inputs.len()), ("and", 2));
    assert!(graph.iter().all(|node| node.problem.is_none()));
}

#[test]
fn the_frame_loop_reloads_a_changed_plugin_once_it_settles() {
    let workspace = Workspace::new("watch");
    let library = workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=1.0f"]);
    let mut app = app();
    let entity = app.world.spawn(Transform::IDENTITY);
    app.load_native_plugin(&library).unwrap();
    app.native_plugins().check_interval = std::time::Duration::ZERO;
    app.update();
    assert_eq!(position(&app, entity).x, 1.0);

    workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=100.0f"]);
    app.update(); // sees the change, waits to see if the file is still being written
    assert_eq!(position(&app, entity).x, 2.0);
    app.update(); // unchanged since last look: reloads, then runs the new code
    assert_eq!(position(&app, entity).x, 102.0);
}

#[test]
fn a_broken_rebuild_leaves_the_old_version_running() {
    let workspace = Workspace::new("broken");
    let library = workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=1.0f"]);
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
    workspace.compile("counter", COUNTER, &["ABI=MIRA_ABI_VERSION", "STEP=5.0f"]);
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

    let conflicting = workspace.compile("conflicting", CONFLICTING, &["ABI=MIRA_ABI_VERSION"]);
    assert!(app.load_native_plugin(&conflicting).is_err());

    // None of that left anything behind.
    app.world.spawn(Transform::IDENTITY);
    app.update();
    assert_eq!(app.native_plugins().loaded().count(), 0);
}

/// Builds one of the workspace's Rust example plugins and says where it is.
fn build_rust_plugin(name: &str) -> PathBuf {
    // Built into its own directory: the outer `cargo test` may hold the lock on the main one.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target").join("plugin-tests");
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", name, "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success(), "building the {name} plugin failed");
    target.join("debug").join(library_file_name(name))
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

    let library = build_rust_plugin("wave");

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
    let library = workspace.compile("game", GAME, &["ABI=MIRA_ABI_VERSION"]);
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

/// The one value of a runtime-defined component that exists, as `T`.
fn only<T: Copy>(app: &App, component: &str) -> (Entity, T) {
    let id = app.world.named_component_id(component).unwrap();
    let storage = app
        .world
        .erased_storage(app.world.named_component(id).unwrap().key)
        .unwrap();
    let [entity] = storage.entities() else {
        panic!("expected exactly one `{component}`");
    };
    // SAFETY: nothing else is using the world, and the test names the component's real type.
    (*entity, unsafe {
        *storage.value_ptr(*entity).unwrap().cast::<T>()
    })
}

#[test]
fn a_plugin_can_own_the_scene_use_physics_and_talk_to_another_plugin() {
    use crate::{
        input::{ButtonInput, InputPlugin, KeyCode},
        physics::PhysicsPlugin,
        render::{AmbientLight, Camera, DirectionalLight},
        time::Time,
    };
    use std::time::Duration;

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct Report {
        contacts: i32,
        speed: f32,
        ray: f32,
        ticks_sent: i32,
    }

    let workspace = Workspace::new("scene");
    let scene = workspace.compile("scene", SCENE, &["ABI=MIRA_ABI_VERSION"]);
    let listener = workspace.compile("listener", LISTENER, &["ABI=MIRA_ABI_VERSION"]);

    let mut app = app();
    app.add_plugins(InputPlugin).add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_fixed_step(Some(Duration::from_secs_f64(1.0 / 60.0)));
    app.load_native_plugin(&scene).unwrap();
    app.load_native_plugin(&listener).unwrap();
    for _ in 0..180 {
        app.update();
    }

    // The scene the plugin set up.
    let camera = *app.world.query::<&Camera>().single();
    assert_eq!((camera.fov_y, camera.near, camera.active), (1.0, 0.5, true));
    let sun = *app.world.query::<&DirectionalLight>().single();
    assert_eq!((sun.intensity, sun.shadows), (4.0, false));
    assert_eq!(sun.color.to_array(), [1.0, 0.5, 0.25, 1.0]);
    let ambient = app.world.resource::<AmbientLight>();
    assert_eq!((ambient.intensity, ambient.color.b), (0.7, 0.3));

    // Physics: the ball fell five metres and came to rest on the ground, and the plugin saw
    // it happen through contact events, its velocity, and a ray cast from above.
    let (ball, report) = only::<Report>(&app, "test.Report");
    let height = position(&app, ball).y;
    assert!(
        (height - 0.5).abs() < 0.05,
        "the ball rests on the ground: y = {height}"
    );
    assert!(report.contacts > 0, "{report:?}");
    assert!(report.speed.abs() < 0.2, "{report:?}");
    assert!((report.ray - 9.0).abs() < 0.1, "{report:?}");

    // Events: the other plugin heard every tick, once each.
    assert_eq!(report.ticks_sent, 180);
    let (_, heard) = only::<[i32; 2]>(&app, "test.Heard");
    assert_eq!(heard, [180, (0..180).sum()]);

    // An impulse, on a key press.
    app.world
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    for _ in 0..10 {
        app.update();
    }
    assert!(position(&app, ball).y > 1.0, "the ball was knocked upward");
}

/// An app with what a plugin needs to load files, rooted at a folder with a picture and a
/// model in it.
fn app_with_assets(workspace: &Workspace) -> App {
    use crate::{
        asset_server::AssetServer,
        assets::Assets,
        render::{Image, Material, Mesh, Mesh3d},
    };
    image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
        .save(workspace.dir.join("tile.png"))
        .unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("res/paris/models/animals/hen_white.glb"),
        workspace.dir.join("hen.glb"),
    )
    .unwrap();
    // A prefab: a camp of two tents.
    let mut camp = World::new();
    camp.spawn(Transform::from_xyz(-1.0, 0.0, 0.0));
    camp.spawn(Transform::from_xyz(1.0, 0.0, 0.0));
    let mut transforms = crate::reflect::TypeRegistry::default();
    transforms.register::<Transform>();
    crate::reflect::Scene::capture(&camp, &transforms)
        .save(workspace.dir.join("camp.json"))
        .unwrap();
    let mut app = app();
    app.add_plugins(crate::input::InputPlugin)
        .add_plugins(crate::prefab::PrefabPlugin);
    app.insert_resource(AssetServer::new(&workspace.dir))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .register_type::<Mesh3d>()
        .register_type::<Material>();
    app
}

#[test]
fn a_described_plugin_component_can_be_inspected_saved_and_loaded() {
    use crate::{
        asset_server::AssetServer,
        assets::Assets,
        reflect::{Scene, Schema, TypeRegistry, Value},
        render::{Image, Material, Mesh3d},
        transform::Parent,
    };

    let workspace = Workspace::new("inventory");
    let library = workspace.compile("inventory", INVENTORY, &["ABI=MIRA_ABI_VERSION"]);
    let mut app = app_with_assets(&workspace);
    app.load_native_plugin(&library).unwrap();
    app.update();
    app.update();

    // The engine knows the shape of a component it has no type for.
    let registry = app.world.resource::<TypeRegistry>();
    let item_type = registry
        .get("inv.Item")
        .expect("the plugin described its component");
    let Schema::Struct { fields, .. } = (item_type.schema)() else {
        panic!("a described component is a struct");
    };
    let Schema::Fields(fields) = *fields else {
        panic!("with fields");
    };
    let names: Vec<_> = fields.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["weight", "count", "rare", "owner", "tint"]);
    assert_eq!(fields[4].1, Schema::Array(Box::new(Schema::Float), 3));

    // And its values, by name.
    let sword = (item_type.entities)(&app.world)[0];
    let item = (item_type.get)(&app.world, sword).unwrap();
    assert_eq!(item.field("weight"), Some(&Value::Float(2.5)));
    assert_eq!(item.field("count"), Some(&Value::Int(3)));
    assert_eq!(item.field("rare"), Some(&Value::Bool(true)));
    let Some(&Value::Entity(owner)) = item.field("owner") else {
        panic!("the owner is an entity");
    };
    assert!(app.world.contains_entity(Entity::from_bits(owner)));
    assert_eq!(
        position(&app, sword).x,
        7.5,
        "the plugin shows weight times count"
    );

    // An inspector edits it, and the plugin acts on the new value.
    let mut edited = item.clone();
    edited.set_path("count", Value::Int(10));
    app.world
        .resource_scope(|world, registry: &mut TypeRegistry| {
            (registry.get("inv.Item").unwrap().insert)(world, sword, &edited).unwrap();
            let bad = Value::Map(vec![("count".into(), Value::Text("many".into()))]);
            assert!((registry.get("inv.Item").unwrap().insert)(world, sword, &bad).is_err());
        });
    app.update();
    assert_eq!(position(&app, sword).x, 25.0);

    // Files: the image it loaded by name, and the model it spawned as a parent with parts.
    app.world.resource_scope(|world, server: &mut AssetServer| {
        server.wait(world.resource_mut::<Assets<Image>>());
    });
    let texture = app
        .world
        .get::<Material>(sword)
        .unwrap()
        .base_color_texture
        .unwrap();
    let server = app.world.resource::<AssetServer>();
    assert_eq!(server.name_of(texture), Some("tile.png"));
    assert_eq!(
        server.name_of(app.world.get::<Mesh3d>(sword).unwrap().0),
        Some("shape:cube:1")
    );
    assert_eq!(
        app.world
            .resource::<Assets<Image>>()
            .get(texture)
            .unwrap()
            .data[..4],
        [10, 20, 30, 255]
    );
    let parts: Vec<Entity> = app
        .world
        .query::<(Entity, &Parent, &Mesh3d)>()
        .iter()
        .map(|(e, ..)| e)
        .collect();
    assert!(
        !parts.is_empty(),
        "the model's parts hang off the entity the plugin was given"
    );
    let root = app.world.get::<Parent>(parts[0]).unwrap().0;
    assert_eq!(position(&app, root).x, 5.0);

    // A prefab: the instance the plugin asked for, with the file's two entities below it.
    let (camp, tents) = app
        .world
        .query::<(Entity, &crate::prefab::PrefabInstance)>()
        .iter()
        .map(|(entity, instance)| (entity, instance.entities().to_vec()))
        .next()
        .expect("the plugin spawned a prefab instance");
    assert_eq!(position(&app, camp).x, 5.0);
    assert_eq!(tents.len(), 2);
    assert!(tents
        .iter()
        .all(|&tent| app.world.get::<Parent>(tent) == Some(&Parent(camp))));

    // Hierarchy: the lantern the plugin hung on the owner is placed relative to it.
    let owner_entity = Entity::from_bits(owner);
    let lantern = app
        .world
        .query::<(Entity, &Parent)>()
        .iter()
        .find(|(_, parent)| parent.0 == owner_entity)
        .map(|(entity, _)| entity)
        .expect("the lantern is the owner's child");
    app.world
        .get_mut::<Transform>(owner_entity)
        .unwrap()
        .translation
        .x = 3.0;
    app.update();
    let lantern_at = app
        .world
        .get::<crate::transform::GlobalTransform>(lantern)
        .unwrap()
        .translation();
    assert_eq!(lantern_at, Vec3::new(3.0, 2.0, 0.0));

    // The whole thing saved, and loaded into another run of the same game.
    let text = Scene::capture(&app.world, app.world.resource::<TypeRegistry>()).to_json();
    assert!(text.contains("\"inv.Item\""), "{text}");
    let other_workspace = Workspace::new("inventory-again");
    let mut other = app_with_assets(&other_workspace);
    other.load_native_plugin(&library).unwrap();
    other.update();
    let spawned = other
        .world
        .resource_scope(|world, registry: &mut TypeRegistry| {
            Scene::from_json(&text).unwrap().spawn(world, registry)
        });
    assert!(spawned.skipped.is_empty(), "{:?}", spawned.skipped);
    let registry = other.world.resource::<TypeRegistry>();
    let item_type = registry.get("inv.Item").unwrap();
    let loaded: Vec<Entity> = (item_type.entities)(&other.world)
        .into_iter()
        .filter(|e| spawned.entities.contains(e))
        .collect();
    let [loaded_sword] = loaded[..] else {
        panic!("one item came from the scene");
    };
    let item = (item_type.get)(&other.world, loaded_sword).unwrap();
    assert_eq!(
        item.field("count"),
        Some(&Value::Int(10)),
        "the edited value was saved"
    );
    assert_eq!(
        item.field("tint"),
        Some(&Value::List(vec![
            Value::Float(0.1f32 as f64),
            Value::Float(0.2f32 as f64),
            Value::Float(0.3f32 as f64)
        ]))
    );
    let Some(&Value::Entity(new_owner)) = item.field("owner") else {
        panic!("the owner is an entity");
    };
    assert_ne!(new_owner, owner);
    assert!(
        spawned.entities.contains(&Entity::from_bits(new_owner)),
        "it points at the loaded owner"
    );

    // Despawning the model's root takes its parts with it.
    assert!(app.world.contains_entity(root));
    app.world
        .resource_mut::<crate::input::ButtonInput<crate::input::KeyCode>>()
        .press(crate::input::KeyCode::Backspace);
    app.update();
    assert!(!app.world.contains_entity(root));
    assert!(parts.iter().all(|part| !app.world.contains_entity(*part)));
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
import Mira

foreign import ccall unsafe "mira_hs_nonmoving_gc" nonmovingGC :: IO CInt

foreign export ccall "mira_hs_main" pluginMain :: Ptr () -> IO CInt

pluginMain :: Ptr () -> IO CInt
pluginMain = plugin $ \app -> do
  -- The old generation must be collected without long pauses: the non-moving collector,
  -- marking concurrently, which needs the threaded runtime.
  nonmoving <- nonmovingGC
  unless (nonmoving /= 0) $ ioError (userError "the non-moving collector is off")
  unless rtsSupportsBoundThreads $ ioError (userError "not the threaded runtime")
  transform <- lookupComponent app "mira.Transform"
  frames <- registerComponent app "hs.Frames" :: IO (Component Int32)
  addSystem app "adopt" Update (with transform <* without frames) $ \sys entity () ->
    insert sys entity frames 0
  addSystem app "step" Update ((,) <$> write transform <*> write frames) $ \_ _ (place, count) -> do
    modifyRef place $ \t -> t {translation = translation t + V3 STEP 0 0}
    modifyRef count (+ 1)
  addSystem_ app "rules" Update $ \sys -> do
    setSignal sys "hs.ready" True
    defineSignal sys "hs.idle" SignalNot ["hs.ready"]
    defineSignal sys "hs.settled" (SignalHeldFor 0) ["hs.ready"]
    ready <- signalIsTrue sys "hs.ready"
    missing <- signal sys "hs.nothing"
    setSignalNumber sys "hs.seen" (if ready && missing == Nothing then 7 else 0)
    -- A rule as an expression; its parts become signals of their own.
    defineRule sys "hs.go" $
      (sig "hs.ready" .&&. notS (sig "hs.idle") .&&. truth True)
        .||. (sig "hs.seen" + 1 .>. number 100)
    defineRule sys "hs.clock" (timer (sig "hs.go"))
    defineRule sys "hs.alias" (sig "hs.go")
    defineRule sys "hs.pick" (selectS (heldFor 0 (sig "hs.go")) (sig "hs.seen" + 3) 0)
    defineRule sys "hs.two" (countS [sig "hs.ready", sig "hs.idle", sig "hs.go"] .==. 2)
  addSystem_ app "churn" Update $ \_ -> do
    let total = sum (map fromIntegral [1 .. 20000 :: Int]) :: Double
    total `seq` performMajorGC
"#;

/// Whether the Haskell tests can run. Without GHC they are skipped, loudly; where the suite
/// must be complete (CI sets `MIRA_REQUIRE_GHC=1`) a missing GHC fails them instead.
fn have_ghc() -> bool {
    if Command::new("ghc").arg("--version").output().is_ok() {
        return true;
    }
    assert!(
        std::env::var("MIRA_REQUIRE_GHC").as_deref() != Ok("1"),
        "GHC is required (MIRA_REQUIRE_GHC=1) but not installed"
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
    // Signals: facts the plugin sets, a rule it defines, and its reading them back.
    let signals = app.world.resource::<crate::signal::Signals>();
    assert!(signals.is_true("hs.ready") && !signals.is_true("hs.idle"));
    assert!(signals.is_true("hs.settled"));
    assert_eq!(signals.number("hs.seen"), 7.0);
    assert!(signals.is_true("hs.go") && signals.is_true("hs.alias") && signals.is_true("hs.two"));
    assert_eq!(signals.number("hs.pick"), 10.0);
    assert!(signals.number("hs.clock") > 0.0);
    let graph = signals.graph();
    let go = graph.iter().find(|node| node.name == "hs.go").unwrap();
    assert_eq!((go.kind.as_str(), go.inputs.as_slice()), ("or", &["hs.go#1".to_owned(), "hs.go#4".to_owned()][..]));
    let and = graph.iter().find(|node| node.name == "hs.go#1").unwrap();
    assert_eq!(and.inputs, ["hs.ready", "hs.go#2", "hs.go#3"], "a chain of `and` is one node");
    assert!(graph.iter().all(|node| node.problem.is_none()), "{graph:?}");

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

    // A version that throws: the game pauses with the exception, and a fixed version
    // carries on from where it stopped.
    use crate::live::Live;
    app.world.resource_mut::<Live>().catch_failures = true;
    build_haskell(&workspace, "(error \"the step went missing\")");
    assert_eq!(app.reload_native_plugins(), 1);
    for _ in 0..3 {
        app.update();
    }
    let live = app.world.resource::<Live>();
    assert!(live.is_paused());
    assert_eq!(live.failures().len(), 1, "{:?}", live.failures());
    let failure = &live.failures()[0];
    assert!(failure.system.starts_with("stepper::"), "{}", failure.system);
    assert!(failure.message.contains("the step went missing"), "{}", failure.message);
    assert_eq!(position(&app, entity).x, expected);
    build_haskell(&workspace, "1");
    assert_eq!(app.reload_native_plugins(), 1);
    app.update();
    assert!(!app.world.resource::<Live>().is_paused());
    assert_eq!(position(&app, entity).x, expected + 1.0);
}

/// The example game, which is written entirely as a Haskell plugin: the scene, drawing, input,
/// two queries, physics, spawning and despawning, through the bindings a plugin author would
/// use. Its score is shown by a separate plugin in Rust that only knows the name of an event.
#[test]
fn the_haskell_game_plays() {
    use crate::{
        assets::Assets,
        input::{ButtonInput, InputPlugin, KeyCode},
        physics::{PhysicsPlugin, RigidBody},
        render::{Camera, DirectionalLight, Material, Mesh, Mesh3d},
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
    app.add_plugins(InputPlugin)
        .add_plugins(PhysicsPlugin)
        .init_resource::<Assets<Mesh>>();
    app.load_native_plugin(workspace.library("chase")).unwrap();
    app.load_native_plugin(build_rust_plugin("scoreboard"))
        .unwrap();
    // A fixed sixtieth of a second per frame, so the test doesn't depend on how fast it runs.
    app.world
        .resource_mut::<Time>()
        .set_fixed_step(Some(Duration::from_secs_f64(1.0 / 60.0)));
    let frame = |app: &mut App| app.update();
    frame(&mut app);
    frame(&mut app);

    let drawn = |app: &mut App| app.world.query::<(&Mesh3d, &Material)>().count();
    assert_eq!(
        drawn(&mut app),
        18,
        "the field, the player, ten pickups and six crates"
    );
    assert_eq!(
        app.world.query::<&Camera>().count(),
        1,
        "the game brings its own camera"
    );
    assert_eq!(app.world.query::<&DirectionalLight>().count(), 1);
    assert_eq!(
        app.world.query::<&RigidBody>().count(),
        7,
        "the player and the crates"
    );
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
    for _ in 0..12 {
        frame(&mut app);
    }
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
    assert_eq!(drawn(&mut app), 18);

    // The game announced it as an event, which anyone who knows the name can read: the
    // scoreboard plugin did, and so can the engine.
    let events = app.world.resource::<crate::plugin::PluginEvents>();
    let collected = events.id("chase.Collected").unwrap();
    let reader = events.reader("the test");
    let latest = events.next(reader, collected).unwrap();
    // At least this one; steering may have run the player over another on the way.
    assert!(i32::from_ne_bytes(latest.try_into().unwrap()) >= 1);

    // Physics the game set up but doesn't drive: after a while every crate is at rest on the
    // ground, wherever it was dropped or pushed.
    for _ in 0..120 {
        frame(&mut app);
    }
    let crates: Vec<f32> = app
        .world
        .query::<(&Transform, &RigidBody)>()
        .iter()
        .filter(|(_, body)| body.kind == crate::physics::BodyKind::Dynamic)
        .map(|(transform, _)| transform.translation.y)
        .collect();
    assert_eq!(crates.len(), 6);
    assert!(crates.iter().all(|y| (y - 0.5).abs() < 0.1), "{crates:?}");
}

/// Two apps on two threads load Haskell plugins at the same instant. Both plugins share one
/// GHC runtime, which can't be started from two threads at once, so the loader has to take
/// them one at a time. (In CI this race once made the suite fail on Linux and crash on macOS.)
#[test]
fn two_haskell_plugins_can_load_at_the_same_moment() {
    if !have_ghc() {
        return;
    }
    let workspace = Workspace::new("together");
    let libraries: Vec<PathBuf> = ["one", "two"]
        .iter()
        .map(|name| {
            let source = workspace.dir.join(format!("{name}.hs"));
            let module = HASKELL
                .replace("STEP", "1")
                .replace("hs.Frames", &format!("{name}.Frames"));
            std::fs::write(&source, module).unwrap();
            let script =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings/haskell/build-plugin.sh");
            let output = Command::new(script)
                .arg(name)
                .arg(&source)
                .arg(&workspace.dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            workspace.library(name)
        })
        .collect();

    let start = std::sync::Barrier::new(libraries.len());
    std::thread::scope(|scope| {
        for library in &libraries {
            let start = &start;
            scope.spawn(move || {
                let mut app = app();
                let entity = app.world.spawn(Transform::IDENTITY);
                start.wait();
                app.load_native_plugin(library).unwrap();
                for _ in 0..10 {
                    app.update();
                }
                assert_eq!(position(&app, entity).x, 10.0);
            });
        }
    });
}
