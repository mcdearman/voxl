/*
 * The native half of the Haskell bindings: compiled into every Haskell plugin.
 *
 * It exports the functions the engine looks for, starts the GHC runtime the first time the
 * plugin is loaded, and gives Haskell ordinary C functions to import instead of a table of
 * function pointers.
 */
#include "HsFFI.h"
#include "Rts.h"

#include <stdlib.h>

#include "mira.h"

/* Exported from Haskell: the plugin's own entry point, and two from the Mira module. */
extern int32_t mira_hs_main(MiraApp *app);
extern void mira_hs_dispatch(MiraSystem *system, void *user);
extern void mira_hs_unload(void);

static const MiraApi *api;
static int runtime_started;

MIRA_EXPORT uint32_t mira_plugin_abi_version(void) { return MIRA_ABI_VERSION; }

/* The GHC runtime's garbage collector keeps pointers into every Haskell library it has run,
 * so an old version of the plugin must stay mapped after a reload. */
MIRA_EXPORT uint32_t mira_plugin_flags(void) { return MIRA_PLUGIN_KEEP_LOADED; }

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *engine, MiraApp *app) {
    api = engine;
    if (!runtime_started) {
        /* The runtime is shared by every Haskell plugin and every reloaded version (they link
         * to it dynamically), and starting it again just counts. It is a guest in the engine's
         * process, so it must not replace the engine's signal handlers. */
        RtsConfig config = defaultRtsConfig;
        /* Never collect top-level values. The runtime only learns a library's exported
         * functions (its collection roots) when the runtime starts, so a version of the plugin
         * loaded later, by a hot reload, would have the values its code refers to collected
         * from under it at the next major collection. */
        config.keep_cafs = 1;
        /* Collect the old generation with the non-moving collector, which marks concurrently
         * on its own thread instead of stopping the game to copy the whole heap: a frame only
         * ever waits for a young-generation collection. This needs the threaded runtime, which
         * build-plugin.sh links. */
        config.rts_opts_enabled = RtsOptsAll;
        config.rts_opts = "--nonmoving-gc --install-signal-handlers=no";
        hs_init_ghc(NULL, NULL, config);
        runtime_started = 1;
    }
    return mira_hs_main(app);
}

MIRA_EXPORT void mira_plugin_unload(void) { mira_hs_unload(); }

/* Whether the old generation is being collected by the non-moving collector, for tests. */
int32_t mira_hs_nonmoving_gc(void) { return RtsFlags.GcFlags.useNonmoving ? 1 : 0; }

void mira_hs_log(uint32_t level, const char *message, size_t len) {
    api->log(level, message, len);
}

uint32_t mira_hs_component_register(MiraApp *app, const char *name, size_t name_len, size_t size,
                                    size_t align) {
    return api->component_register(app, name, name_len, size, align, NULL);
}

uint32_t mira_hs_component_lookup(MiraApp *app, const char *name, size_t name_len, size_t *size,
                                  size_t *align) {
    return api->component_lookup(app, name, name_len, size, align);
}

void *mira_hs_state(MiraApp *app, const char *name, size_t name_len, size_t size, size_t align) {
    return api->state(app, name, name_len, size, align);
}

/* `closure` is a stable pointer to the Haskell function; every system runs through the one
 * dispatcher, which looks it up and calls it. */
int32_t mira_hs_system_add(MiraApp *app, const char *name, size_t name_len, uint32_t stage,
                           void *closure, const MiraTerm *terms, size_t term_count) {
    MiraSystemDesc desc = {
        .name = name,
        .name_len = name_len,
        .stage = stage,
        .run = mira_hs_dispatch,
        .user = closure,
        .terms = terms,
        .term_count = term_count,
    };
    return api->system_add(app, &desc);
}

float mira_hs_delta_seconds(MiraSystem *system) { return api->delta_seconds(system); }
double mira_hs_elapsed_seconds(MiraSystem *system) { return api->elapsed_seconds(system); }

uint8_t mira_hs_query_next(MiraSystem *system, MiraEntity *entity, void **components) {
    return api->query_next(system, entity, components);
}

uint8_t mira_hs_query_get(MiraSystem *system, MiraEntity entity, void **components) {
    return api->query_get(system, entity, components);
}

MiraEntity mira_hs_spawn(MiraSystem *system) { return api->spawn(system); }
void mira_hs_despawn(MiraSystem *system, MiraEntity entity) { api->despawn(system, entity); }

void mira_hs_insert(MiraSystem *system, MiraEntity entity, uint32_t component, const void *value) {
    api->insert(system, entity, component, value);
}

void mira_hs_remove(MiraSystem *system, MiraEntity entity, uint32_t component) {
    api->remove(system, entity, component);
}

/* Adds a system with any number of queries: `counts[i]` terms for query i, laid end to end in
 * `terms`. Query 0 is the system's own; the rest are added after it. */
int32_t mira_hs_system_add_queries(MiraApp *app, const char *name, size_t name_len, uint32_t stage,
                                   void *closure, const MiraTerm *terms, const size_t *counts,
                                   size_t query_count) {
    int32_t status = mira_hs_system_add(app, name, name_len, stage, closure, terms,
                                        query_count ? counts[0] : 0);
    if (status != 0) return status;
    for (size_t query = 1; query < query_count; query++) {
        terms += counts[query - 1];
        if (api->system_add_query(app, name, name_len, terms, counts[query]) != (int32_t)query)
            return -1;
    }
    return 0;
}

uint8_t mira_hs_query_next_in(MiraSystem *system, uint32_t query, MiraEntity *entity,
                              void **components) {
    return api->query_next_in(system, query, entity, components);
}

uint8_t mira_hs_query_get_in(MiraSystem *system, uint32_t query, MiraEntity entity,
                             void **components) {
    return api->query_get_in(system, query, entity, components);
}

void mira_hs_query_rewind(MiraSystem *system, uint32_t query) { api->query_rewind(system, query); }

uint8_t mira_hs_key_down(MiraSystem *system, uint32_t key) { return api->key_down(system, key); }
uint8_t mira_hs_key_pressed(MiraSystem *system, uint32_t key) {
    return api->key_pressed(system, key);
}
uint8_t mira_hs_key_released(MiraSystem *system, uint32_t key) {
    return api->key_released(system, key);
}
uint8_t mira_hs_mouse_down(MiraSystem *system, uint32_t button) {
    return api->mouse_down(system, button);
}
uint8_t mira_hs_mouse_pressed(MiraSystem *system, uint32_t button) {
    return api->mouse_pressed(system, button);
}
void mira_hs_mouse_motion(MiraSystem *system, float *delta) { api->mouse_motion(system, delta); }

MiraMesh mira_hs_mesh_shape(MiraSystem *system, uint32_t shape, float a) {
    return api->mesh_shape(system, shape, a);
}

MiraMesh mira_hs_mesh_create(MiraSystem *system, const MiraVertex *vertices, size_t vertex_count,
                             const uint32_t *indices, size_t index_count) {
    return api->mesh_create(system, vertices, vertex_count, indices, index_count);
}

void mira_hs_set_mesh(MiraSystem *system, MiraEntity entity, MiraMesh mesh) {
    api->set_mesh(system, entity, mesh);
}

void mira_hs_set_material(MiraSystem *system, MiraEntity entity, const MiraMaterial *material) {
    api->set_material(system, entity, material);
}

uint32_t mira_hs_event_register(MiraApp *app, const char *name, size_t name_len, size_t size) {
    return api->event_register(app, name, name_len, size);
}

void mira_hs_event_send(MiraSystem *system, uint32_t event, const void *value) {
    api->event_send(system, event, value);
}

uint8_t mira_hs_event_next(MiraSystem *system, uint32_t event, void *value) {
    return api->event_next(system, event, value);
}

/* The structures below are small, so Haskell passes their fields and they are assembled here:
 * one definition of each layout (the header's) instead of a second one in Haskell. */

void mira_hs_set_camera(MiraSystem *system, MiraEntity entity, float fov_y, float near,
                        uint32_t active) {
    MiraCamera camera = {fov_y, near, active};
    api->set_camera(system, entity, &camera);
}

void mira_hs_set_light(MiraSystem *system, MiraEntity entity, float r, float g, float b,
                       float intensity, uint32_t shadows) {
    MiraLight light = {{r, g, b}, intensity, shadows};
    api->set_light(system, entity, &light);
}

void mira_hs_set_ambient(MiraSystem *system, float r, float g, float b, float intensity) {
    float color[3] = {r, g, b};
    api->set_ambient(system, color, intensity);
}

void mira_hs_set_window_title(MiraSystem *system, const char *title, size_t len) {
    api->set_window_title(system, title, len);
}

void mira_hs_set_collider(MiraSystem *system, MiraEntity entity, uint32_t shape, float x, float y,
                          float z, float friction, float restitution, uint32_t sensor) {
    MiraCollider collider = {shape, {x, y, z}, friction, restitution, sensor};
    api->set_collider(system, entity, &collider);
}

void mira_hs_set_body(MiraSystem *system, MiraEntity entity, uint32_t kind, float mass, float vx,
                      float vy, float vz, uint32_t lock_rotation) {
    MiraBody body = {kind, mass, {vx, vy, vz}, lock_rotation};
    api->set_body(system, entity, &body);
}

void mira_hs_apply_impulse(MiraSystem *system, MiraEntity entity, float x, float y, float z) {
    float impulse[3] = {x, y, z};
    api->apply_impulse(system, entity, impulse);
}

void mira_hs_set_velocity(MiraSystem *system, MiraEntity entity, float x, float y, float z) {
    float velocity[3] = {x, y, z};
    api->set_velocity(system, entity, velocity);
}

uint8_t mira_hs_velocity(MiraSystem *system, MiraEntity entity, float *velocity) {
    return api->velocity(system, entity, velocity);
}

/* `out` receives seven floats: the point, the normal, and the distance. */
uint8_t mira_hs_raycast(MiraSystem *system, float ox, float oy, float oz, float dx, float dy,
                        float dz, float max_distance, MiraEntity *entity, float *out) {
    float origin[3] = {ox, oy, oz}, direction[3] = {dx, dy, dz};
    MiraRayHit hit;
    if (!api->raycast(system, origin, direction, max_distance, &hit)) return 0;
    *entity = hit.entity;
    for (int i = 0; i < 3; i++) {
        out[i] = hit.point[i];
        out[3 + i] = hit.normal[i];
    }
    out[6] = hit.distance;
    return 1;
}

/* Field names arrive end to end in `names`, with their lengths alongside. */
int32_t mira_hs_component_describe(MiraApp *app, uint32_t component, const char *names,
                                   const size_t *name_lens, const uint32_t *types,
                                   const uint32_t *counts, const size_t *offsets, size_t count) {
    MiraField *fields = calloc(count ? count : 1, sizeof(MiraField));
    if (!fields) return -1;
    for (size_t i = 0; i < count; i++) {
        fields[i].name = names;
        fields[i].name_len = name_lens[i];
        fields[i].type = types[i];
        fields[i].count = counts[i];
        fields[i].offset = offsets[i];
        names += name_lens[i];
    }
    int32_t status = api->component_describe(app, component, fields, count);
    free(fields);
    return status;
}

uint32_t mira_hs_image_load(MiraSystem *system, const char *name, size_t len) {
    return api->image_load(system, name, len);
}

void mira_hs_set_textures(MiraSystem *system, MiraEntity entity, uint32_t base_color,
                          uint32_t normal, uint32_t metallic_roughness) {
    api->set_textures(system, entity, base_color, normal, metallic_roughness);
}

MiraEntity mira_hs_spawn_model(MiraSystem *system, const char *name, size_t len,
                               const MiraTransform *transform) {
    return api->spawn_model(system, name, len, transform);
}

void mira_hs_set_parent(MiraSystem *system, MiraEntity child, MiraEntity parent) {
    api->set_parent(system, child, parent);
}

void mira_hs_despawn_tree(MiraSystem *system, MiraEntity entity) {
    api->despawn_tree(system, entity);
}

MiraEntity mira_hs_spawn_prefab(MiraSystem *system, const char *name, size_t len,
                                const MiraTransform *transform) {
    return api->spawn_prefab(system, name, len, transform);
}

void mira_hs_system_fail(MiraSystem *system, const char *message, size_t len) {
    api->system_fail(system, message, len, "", 0);
}

void mira_hs_signal_set(MiraSystem *system, const char *name, size_t len, double value,
                        uint32_t number) {
    api->signal_set(system, name, len, value, number);
}

uint32_t mira_hs_signal_get(MiraSystem *system, const char *name, size_t len, double *out) {
    return api->signal_get(system, name, len, out);
}

void mira_hs_signal_define(MiraSystem *system, const char *name, size_t len, uint32_t op,
                           double param, const char *inputs, size_t inputs_len) {
    api->signal_define(system, name, len, op, param, inputs, inputs_len);
}
