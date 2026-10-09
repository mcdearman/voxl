/*
 * An example mira plugin in C: makes everything with a `demo.Cell` swell and shrink.
 *
 * Build it next to the Rust plugins (from the repository root):
 *
 *     plugins/pulse/build.sh
 *
 * Run `cargo run --example plugins`, change a number below, and build again: the running app
 * picks up the new code.
 */
#include <math.h>

#include "mira.h"

#define AMOUNT 0.25f
#define RATE 3.0f

typedef struct {
    float x, z;
} Cell;

static const MiraApi *api;

static void pulse(MiraSystem *system, void *user) {
    (void)user;
    float time = (float)api->elapsed_seconds(system);
    MiraEntity entity;
    void *found[2];
    while (api->query_next(system, &entity, found)) {
        MiraTransform *transform = found[0];
        const Cell *cell = found[1];
        float size = 0.8f + AMOUNT * sinf(time * RATE + cell->x * 0.7f - cell->z * 0.4f);
        transform->scale[0] = transform->scale[1] = transform->scale[2] = size;
    }
}

MIRA_EXPORT uint32_t mira_plugin_abi_version(void) { return MIRA_ABI_VERSION; }

MIRA_EXPORT int32_t mira_plugin_load(const MiraApi *engine, MiraApp *app) {
    api = engine;

    size_t size = 0, align = 0;
    MiraComponent transform =
        api->component_lookup(app, MIRA_STR("mira.Transform"), &size, &align);
    if (!transform || size != sizeof(MiraTransform) || align != _Alignof(MiraTransform)) {
        api->log(MIRA_LOG_ERROR, MIRA_STR("pulse: mira.Transform is missing or has changed"));
        return -1;
    }
    MiraComponent cell = api->component_lookup(app, MIRA_STR("demo.Cell"), &size, NULL);
    if (!cell || size != sizeof(Cell)) {
        api->log(MIRA_LOG_ERROR, MIRA_STR("pulse: demo.Cell is missing or has changed"));
        return -1;
    }

    MiraTerm terms[] = {{transform, MIRA_WRITE}, {cell, MIRA_READ}};
    MiraSystemDesc desc = {
        .name = "pulse",
        .name_len = 5,
        .stage = MIRA_STAGE_UPDATE,
        .run = pulse,
        .terms = terms,
        .term_count = 2,
    };
    return api->system_add(app, &desc);
}

MIRA_EXPORT void mira_plugin_unload(void) { api = NULL; }
