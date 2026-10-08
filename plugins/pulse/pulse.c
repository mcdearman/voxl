/*
 * An example voxl plugin in C: makes everything with a `demo.Cell` swell and shrink.
 *
 * Build it next to the Rust plugins (from the repository root):
 *
 *     plugins/pulse/build.sh
 *
 * Run `cargo run --example plugins`, change a number below, and build again: the running app
 * picks up the new code.
 */
#include <math.h>

#include "voxl.h"

#define AMOUNT 0.25f
#define RATE 3.0f

typedef struct {
    float x, z;
} Cell;

static const VoxlApi *api;

static void pulse(VoxlSystem *system, void *user) {
    (void)user;
    float time = (float)api->elapsed_seconds(system);
    VoxlEntity entity;
    void *found[2];
    while (api->query_next(system, &entity, found)) {
        VoxlTransform *transform = found[0];
        const Cell *cell = found[1];
        float size = 0.8f + AMOUNT * sinf(time * RATE + cell->x * 0.7f - cell->z * 0.4f);
        transform->scale[0] = transform->scale[1] = transform->scale[2] = size;
    }
}

VOXL_EXPORT uint32_t voxl_plugin_abi_version(void) { return VOXL_ABI_VERSION; }

VOXL_EXPORT int32_t voxl_plugin_load(const VoxlApi *engine, VoxlApp *app) {
    api = engine;

    size_t size = 0, align = 0;
    VoxlComponent transform =
        api->component_lookup(app, VOXL_STR("voxl.Transform"), &size, &align);
    if (!transform || size != sizeof(VoxlTransform) || align != _Alignof(VoxlTransform)) {
        api->log(VOXL_LOG_ERROR, VOXL_STR("pulse: voxl.Transform is missing or has changed"));
        return -1;
    }
    VoxlComponent cell = api->component_lookup(app, VOXL_STR("demo.Cell"), &size, NULL);
    if (!cell || size != sizeof(Cell)) {
        api->log(VOXL_LOG_ERROR, VOXL_STR("pulse: demo.Cell is missing or has changed"));
        return -1;
    }

    VoxlTerm terms[] = {{transform, VOXL_WRITE}, {cell, VOXL_READ}};
    VoxlSystemDesc desc = {
        .name = "pulse",
        .name_len = 5,
        .stage = VOXL_STAGE_UPDATE,
        .run = pulse,
        .terms = terms,
        .term_count = 2,
    };
    return api->system_add(app, &desc);
}

VOXL_EXPORT void voxl_plugin_unload(void) { api = NULL; }
