/*
 * The native half of the Haskell bindings: compiled into every Haskell plugin.
 *
 * It exports the functions the engine looks for, starts the GHC runtime the first time the
 * plugin is loaded, and gives Haskell ordinary C functions to import instead of a table of
 * function pointers.
 */
#include "HsFFI.h"
#include "Rts.h"

#include "voxl.h"

/* Exported from Haskell: the plugin's own entry point, and two from the Voxl module. */
extern int32_t voxl_hs_main(VoxlApp *app);
extern void voxl_hs_dispatch(VoxlSystem *system, void *user);
extern void voxl_hs_unload(void);

static const VoxlApi *api;
static int runtime_started;

VOXL_EXPORT uint32_t voxl_plugin_abi_version(void) { return VOXL_ABI_VERSION; }

/* The GHC runtime's garbage collector keeps pointers into every Haskell library it has run,
 * so an old version of the plugin must stay mapped after a reload. */
VOXL_EXPORT uint32_t voxl_plugin_flags(void) { return VOXL_PLUGIN_KEEP_LOADED; }

VOXL_EXPORT int32_t voxl_plugin_load(const VoxlApi *engine, VoxlApp *app) {
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
    return voxl_hs_main(app);
}

VOXL_EXPORT void voxl_plugin_unload(void) { voxl_hs_unload(); }

/* Whether the old generation is being collected by the non-moving collector, for tests. */
int32_t voxl_hs_nonmoving_gc(void) { return RtsFlags.GcFlags.useNonmoving ? 1 : 0; }

void voxl_hs_log(uint32_t level, const char *message, size_t len) {
    api->log(level, message, len);
}

uint32_t voxl_hs_component_register(VoxlApp *app, const char *name, size_t name_len, size_t size,
                                    size_t align) {
    return api->component_register(app, name, name_len, size, align, NULL);
}

uint32_t voxl_hs_component_lookup(VoxlApp *app, const char *name, size_t name_len, size_t *size,
                                  size_t *align) {
    return api->component_lookup(app, name, name_len, size, align);
}

void *voxl_hs_state(VoxlApp *app, const char *name, size_t name_len, size_t size, size_t align) {
    return api->state(app, name, name_len, size, align);
}

/* `closure` is a stable pointer to the Haskell function; every system runs through the one
 * dispatcher, which looks it up and calls it. */
int32_t voxl_hs_system_add(VoxlApp *app, const char *name, size_t name_len, uint32_t stage,
                           void *closure, const VoxlTerm *terms, size_t term_count) {
    VoxlSystemDesc desc = {
        .name = name,
        .name_len = name_len,
        .stage = stage,
        .run = voxl_hs_dispatch,
        .user = closure,
        .terms = terms,
        .term_count = term_count,
    };
    return api->system_add(app, &desc);
}

float voxl_hs_delta_seconds(VoxlSystem *system) { return api->delta_seconds(system); }
double voxl_hs_elapsed_seconds(VoxlSystem *system) { return api->elapsed_seconds(system); }

uint8_t voxl_hs_query_next(VoxlSystem *system, VoxlEntity *entity, void **components) {
    return api->query_next(system, entity, components);
}

uint8_t voxl_hs_query_get(VoxlSystem *system, VoxlEntity entity, void **components) {
    return api->query_get(system, entity, components);
}

VoxlEntity voxl_hs_spawn(VoxlSystem *system) { return api->spawn(system); }
void voxl_hs_despawn(VoxlSystem *system, VoxlEntity entity) { api->despawn(system, entity); }

void voxl_hs_insert(VoxlSystem *system, VoxlEntity entity, uint32_t component, const void *value) {
    api->insert(system, entity, component, value);
}

void voxl_hs_remove(VoxlSystem *system, VoxlEntity entity, uint32_t component) {
    api->remove(system, entity, component);
}

/* Adds a system with any number of queries: `counts[i]` terms for query i, laid end to end in
 * `terms`. Query 0 is the system's own; the rest are added after it. */
int32_t voxl_hs_system_add_queries(VoxlApp *app, const char *name, size_t name_len, uint32_t stage,
                                   void *closure, const VoxlTerm *terms, const size_t *counts,
                                   size_t query_count) {
    int32_t status = voxl_hs_system_add(app, name, name_len, stage, closure, terms,
                                        query_count ? counts[0] : 0);
    if (status != 0) return status;
    for (size_t query = 1; query < query_count; query++) {
        terms += counts[query - 1];
        if (api->system_add_query(app, name, name_len, terms, counts[query]) != (int32_t)query)
            return -1;
    }
    return 0;
}

uint8_t voxl_hs_query_next_in(VoxlSystem *system, uint32_t query, VoxlEntity *entity,
                              void **components) {
    return api->query_next_in(system, query, entity, components);
}

uint8_t voxl_hs_query_get_in(VoxlSystem *system, uint32_t query, VoxlEntity entity,
                             void **components) {
    return api->query_get_in(system, query, entity, components);
}

void voxl_hs_query_rewind(VoxlSystem *system, uint32_t query) { api->query_rewind(system, query); }

uint8_t voxl_hs_key_down(VoxlSystem *system, uint32_t key) { return api->key_down(system, key); }
uint8_t voxl_hs_key_pressed(VoxlSystem *system, uint32_t key) {
    return api->key_pressed(system, key);
}
uint8_t voxl_hs_key_released(VoxlSystem *system, uint32_t key) {
    return api->key_released(system, key);
}
uint8_t voxl_hs_mouse_down(VoxlSystem *system, uint32_t button) {
    return api->mouse_down(system, button);
}
uint8_t voxl_hs_mouse_pressed(VoxlSystem *system, uint32_t button) {
    return api->mouse_pressed(system, button);
}
void voxl_hs_mouse_motion(VoxlSystem *system, float *delta) { api->mouse_motion(system, delta); }

VoxlMesh voxl_hs_mesh_shape(VoxlSystem *system, uint32_t shape, float a) {
    return api->mesh_shape(system, shape, a);
}

VoxlMesh voxl_hs_mesh_create(VoxlSystem *system, const VoxlVertex *vertices, size_t vertex_count,
                             const uint32_t *indices, size_t index_count) {
    return api->mesh_create(system, vertices, vertex_count, indices, index_count);
}

void voxl_hs_set_mesh(VoxlSystem *system, VoxlEntity entity, VoxlMesh mesh) {
    api->set_mesh(system, entity, mesh);
}

void voxl_hs_set_material(VoxlSystem *system, VoxlEntity entity, const VoxlMaterial *material) {
    api->set_material(system, entity, material);
}

uint32_t voxl_hs_event_register(VoxlApp *app, const char *name, size_t name_len, size_t size) {
    return api->event_register(app, name, name_len, size);
}

void voxl_hs_event_send(VoxlSystem *system, uint32_t event, const void *value) {
    api->event_send(system, event, value);
}

uint8_t voxl_hs_event_next(VoxlSystem *system, uint32_t event, void *value) {
    return api->event_next(system, event, value);
}

/* The structures below are small, so Haskell passes their fields and they are assembled here:
 * one definition of each layout (the header's) instead of a second one in Haskell. */

void voxl_hs_set_camera(VoxlSystem *system, VoxlEntity entity, float fov_y, float near,
                        uint32_t active) {
    VoxlCamera camera = {fov_y, near, active};
    api->set_camera(system, entity, &camera);
}

void voxl_hs_set_light(VoxlSystem *system, VoxlEntity entity, float r, float g, float b,
                       float intensity, uint32_t shadows) {
    VoxlLight light = {{r, g, b}, intensity, shadows};
    api->set_light(system, entity, &light);
}

void voxl_hs_set_ambient(VoxlSystem *system, float r, float g, float b, float intensity) {
    float color[3] = {r, g, b};
    api->set_ambient(system, color, intensity);
}

void voxl_hs_set_window_title(VoxlSystem *system, const char *title, size_t len) {
    api->set_window_title(system, title, len);
}

void voxl_hs_set_collider(VoxlSystem *system, VoxlEntity entity, uint32_t shape, float x, float y,
                          float z, float friction, float restitution, uint32_t sensor) {
    VoxlCollider collider = {shape, {x, y, z}, friction, restitution, sensor};
    api->set_collider(system, entity, &collider);
}

void voxl_hs_set_body(VoxlSystem *system, VoxlEntity entity, uint32_t kind, float mass, float vx,
                      float vy, float vz, uint32_t lock_rotation) {
    VoxlBody body = {kind, mass, {vx, vy, vz}, lock_rotation};
    api->set_body(system, entity, &body);
}

void voxl_hs_apply_impulse(VoxlSystem *system, VoxlEntity entity, float x, float y, float z) {
    float impulse[3] = {x, y, z};
    api->apply_impulse(system, entity, impulse);
}

void voxl_hs_set_velocity(VoxlSystem *system, VoxlEntity entity, float x, float y, float z) {
    float velocity[3] = {x, y, z};
    api->set_velocity(system, entity, velocity);
}

uint8_t voxl_hs_velocity(VoxlSystem *system, VoxlEntity entity, float *velocity) {
    return api->velocity(system, entity, velocity);
}

/* `out` receives seven floats: the point, the normal, and the distance. */
uint8_t voxl_hs_raycast(VoxlSystem *system, float ox, float oy, float oz, float dx, float dy,
                        float dz, float max_distance, VoxlEntity *entity, float *out) {
    float origin[3] = {ox, oy, oz}, direction[3] = {dx, dy, dz};
    VoxlRayHit hit;
    if (!api->raycast(system, origin, direction, max_distance, &hit)) return 0;
    *entity = hit.entity;
    for (int i = 0; i < 3; i++) {
        out[i] = hit.point[i];
        out[3 + i] = hit.normal[i];
    }
    out[6] = hit.distance;
    return 1;
}
