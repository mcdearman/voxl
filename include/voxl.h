/*
 * voxl native plugin interface, version 1.
 *
 * A plugin is a shared library (.dylib / .so / .dll) written in any language that can export
 * C functions. It exports three symbols:
 *
 *     uint32_t voxl_plugin_abi_version(void);   // return VOXL_ABI_VERSION
 *     int32_t  voxl_plugin_load(const VoxlApi *api, VoxlApp *app);   // 0 on success
 *     void     voxl_plugin_unload(void);        // optional
 *     uint32_t voxl_plugin_flags(void);         // optional; VOXL_PLUGIN_* flags
 *
 * `voxl_plugin_load` is called when the plugin is first loaded and again every time it is
 * hot-reloaded. In it, register components and systems through `api`. Keep the `api` pointer:
 * it stays valid until `voxl_plugin_unload` returns.
 *
 * What survives a reload: every component value, and every block handed out by `state`. What
 * does not: the plugin's own globals. Put anything that must persist in a component or state.
 *
 * Rules:
 *   - Functions taking a VoxlApp* may only be called inside voxl_plugin_load.
 *   - Functions taking a VoxlSystem* may only be called inside that system's `run` callback.
 *   - Everything happens on the thread that called you. Do not keep VoxlApp* or VoxlSystem*.
 *   - Component pointers from a query are valid until the system returns.
 *   - Strings are UTF-8 with an explicit length; they need not be NUL-terminated.
 */
#ifndef VOXL_H
#define VOXL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define VOXL_ABI_VERSION 1u

/* Returned by the optional voxl_plugin_flags.
 *
 * KEEP_LOADED: never unmap this library, even after a newer version replaces it. For
 * languages whose runtime can't be unloaded (Haskell, Go): their garbage collector keeps
 * pointers into every library it has run. Old versions are no longer called; they only cost
 * memory, once per reload. */
#define VOXL_PLUGIN_KEEP_LOADED 1u

/* An entity handle. Stays valid until the entity is despawned; never reused afterwards. */
typedef uint64_t VoxlEntity;
#define VOXL_ENTITY_NONE UINT64_MAX

/* A component handle, from component_register or component_lookup. 0 is "no component". */
typedef uint32_t VoxlComponent;

typedef struct VoxlApp VoxlApp;       /* opaque: the app being set up */
typedef struct VoxlSystem VoxlSystem; /* opaque: one run of one system */

/* When in the frame a system runs. */
enum {
    VOXL_STAGE_STARTUP = 0,      /* once, the first time the plugin is loaded */
    VOXL_STAGE_FIRST = 1,
    VOXL_STAGE_PRE_UPDATE = 2,
    VOXL_STAGE_FIXED_UPDATE = 3, /* fixed timestep; may run 0..n times per frame */
    VOXL_STAGE_UPDATE = 4,
    VOXL_STAGE_POST_UPDATE = 5,
    VOXL_STAGE_LAST = 6
};

/* How a system uses a component. READ and WRITE terms yield a pointer per entity, in the
 * order they are listed; WITH and WITHOUT only filter. WRITE marks the value as changed. */
enum {
    VOXL_READ = 0,
    VOXL_WRITE = 1,
    VOXL_WITH = 2,
    VOXL_WITHOUT = 3
};

/* Keys, by position on a US keyboard (so WASD is the same four keys on every layout). */
enum {
    VOXL_KEY_A = 0, /* letters are consecutive: VOXL_KEY_A + ('w' - 'a') is W */
    VOXL_KEY_Z = 25,
    VOXL_KEY_0 = 26, /* the digit row, consecutive */
    VOXL_KEY_9 = 35,
    VOXL_KEY_SPACE = 36,
    VOXL_KEY_ENTER = 37,
    VOXL_KEY_ESCAPE = 38,
    VOXL_KEY_TAB = 39,
    VOXL_KEY_BACKSPACE = 40,
    VOXL_KEY_LEFT = 41,
    VOXL_KEY_RIGHT = 42,
    VOXL_KEY_UP = 43,
    VOXL_KEY_DOWN = 44,
    VOXL_KEY_LEFT_SHIFT = 45,
    VOXL_KEY_RIGHT_SHIFT = 46,
    VOXL_KEY_LEFT_CONTROL = 47,
    VOXL_KEY_RIGHT_CONTROL = 48,
    VOXL_KEY_LEFT_ALT = 49,
    VOXL_KEY_RIGHT_ALT = 50,
    VOXL_KEY_F1 = 51, /* F1 to F12, consecutive */
    VOXL_KEY_F12 = 62
};

enum {
    VOXL_MOUSE_LEFT = 0,
    VOXL_MOUSE_RIGHT = 1,
    VOXL_MOUSE_MIDDLE = 2
};

/* A mesh handle. 0 is "no mesh". */
typedef uint32_t VoxlMesh;

enum {
    VOXL_SHAPE_CUBE = 0,   /* a: edge length */
    VOXL_SHAPE_SPHERE = 1, /* a: radius */
    VOXL_SHAPE_PLANE = 2   /* a: edge length; flat, facing up */
};

/* How a signal is worked out from its inputs, for `signal_define`. */
enum {
    VOXL_SIGNAL_AND = 0,
    VOXL_SIGNAL_OR = 1,
    VOXL_SIGNAL_NOT = 2,
    VOXL_SIGNAL_COUNT = 3,            /* how many inputs are true */
    VOXL_SIGNAL_SUM = 4,
    VOXL_SIGNAL_SELECT = 5,           /* input 1 if input 0 is true, else input 2 */
    VOXL_SIGNAL_TIMER = 6,            /* seconds input 0 has been true; input 1 resets */
    VOXL_SIGNAL_HELD_FOR = 7,         /* true once input 0 has been true for `param` seconds */
    VOXL_SIGNAL_LESS = 8,             /* input 0 against input 1, and so on below */
    VOXL_SIGNAL_LESS_OR_EQUAL = 9,
    VOXL_SIGNAL_EQUAL = 10,
    VOXL_SIGNAL_GREATER_OR_EQUAL = 11,
    VOXL_SIGNAL_GREATER = 12
};

typedef struct VoxlVertex {
    float position[3];
    float normal[3];
    float uv[2];
} VoxlVertex;

/* How a surface looks. Colors are linear RGB(A). */
typedef struct VoxlMaterial {
    float color[4];
    float emissive[3]; /* light the surface gives off itself */
    float roughness;   /* 0 mirror-smooth .. 1 matte */
    float metallic;    /* 0 or 1, usually */
} VoxlMaterial;

/* What one field of a component holds, for component_describe. */
enum {
    VOXL_FIELD_F32 = 0,
    VOXL_FIELD_F64 = 1,
    VOXL_FIELD_I32 = 2,
    VOXL_FIELD_I64 = 3,
    VOXL_FIELD_U8 = 4,
    VOXL_FIELD_U32 = 5,
    VOXL_FIELD_BOOL = 6,  /* one byte; zero is false */
    VOXL_FIELD_ENTITY = 7 /* a VoxlEntity; scenes keep it pointing at the right entity */
};

/* One field of a component: `count` values of one type (more than one for a vector or an
 * array), starting `offset` bytes into the component. */
typedef struct VoxlField {
    const char *name;
    size_t name_len;
    uint32_t type;
    uint32_t count;
    size_t offset;
} VoxlField;

/* An image handle, from image_load. 0 is "no image". */
typedef uint32_t VoxlImage;

/* An event type handle, from event_register. 0 is "no event". */
typedef uint32_t VoxlEvent;

typedef struct VoxlCamera {
    float fov_y;     /* vertical field of view, in radians */
    float near;      /* nearest distance drawn */
    uint32_t active; /* the first active camera is the one that is drawn */
} VoxlCamera;

/* A sun-like light, shining along the forward direction (-Z) of the entity's transform. */
typedef struct VoxlLight {
    float color[3];
    float intensity;
    uint32_t shadows;
} VoxlLight;

enum {
    VOXL_COLLIDER_SPHERE = 0,  /* size[0]: radius */
    VOXL_COLLIDER_BOX = 1,     /* size: half extents along x, y, z */
    VOXL_COLLIDER_CAPSULE = 2, /* size[0]: radius, size[1]: height; upright */
    VOXL_COLLIDER_GROUND = 3   /* everything below the entity's position */
};

/* Makes an entity solid. Without a body it never moves. */
typedef struct VoxlCollider {
    uint32_t shape;
    float size[3];
    float friction;
    float restitution; /* bounciness, 0 to 1 */
    uint32_t sensor;   /* nonzero: detects overlaps but blocks nothing */
} VoxlCollider;

enum {
    VOXL_BODY_DYNAMIC = 0,   /* moved by gravity, forces and collisions */
    VOXL_BODY_KINEMATIC = 1, /* moved only by its velocity; pushes, is never pushed */
    VOXL_BODY_ANIMATED = 2   /* moved by setting its transform; pushes, is never pushed */
};

/* Makes an entity with a collider move. */
typedef struct VoxlBody {
    uint32_t kind;
    float mass; /* kilograms; 0 to work it out from the collider's size */
    float velocity[3];
    uint32_t lock_rotation; /* nonzero keeps it upright */
} VoxlBody;

typedef struct VoxlRayHit {
    VoxlEntity entity;
    float point[3];
    float normal[3];
    float distance;
} VoxlRayHit;

/* The engine's "voxl.Contact" event: two colliders touched during a physics step. */
typedef struct VoxlContact {
    VoxlEntity a;
    VoxlEntity b;
    float point[3];
    float normal[3]; /* from a toward b */
    float impulse;   /* how hard, in newton-seconds */
} VoxlContact;

enum {
    VOXL_LOG_ERROR = 1,
    VOXL_LOG_WARN = 2,
    VOXL_LOG_INFO = 3,
    VOXL_LOG_DEBUG = 4
};

typedef struct VoxlTerm {
    VoxlComponent component;
    uint32_t access; /* VOXL_READ, VOXL_WRITE, VOXL_WITH or VOXL_WITHOUT */
} VoxlTerm;

typedef void (*VoxlSystemFn)(VoxlSystem *system, void *user);

typedef struct VoxlSystemDesc {
    const char *name; /* unique within the plugin; a reloaded system replaces its namesake */
    size_t name_len;
    uint32_t stage;
    uint32_t reserved; /* set to 0 */
    VoxlSystemFn run;
    void *user; /* passed back to `run` */
    const VoxlTerm *terms; /* the entities the system visits; may be empty */
    size_t term_count;
} VoxlSystemDesc;

/* ---- components the engine exports ---- */

/* "voxl.Transform": position, rotation (a unit quaternion, x y z w) and scale, relative to the
 * entity's parent. 48 bytes, 16-byte aligned. */
typedef struct VoxlTransform {
    float translation[3];
    float _pad0;
#if defined(__cplusplus)
    alignas(16) float rotation[4];
#else
    _Alignas(16) float rotation[4];
#endif
    float scale[3];
    float _pad1;
} VoxlTransform;

/* The functions the engine provides. `size` is sizeof(VoxlApi) as the engine sees it; a newer
 * engine may append functions, so check `size` before using one that is not in version 1. */
typedef struct VoxlApi {
    uint32_t abi_version;
    uint32_t size;

    void (*log)(uint32_t level, const char *message, size_t len);

    /* ---- inside voxl_plugin_load ---- */

    /* Defines a component, or finds the one this name already has. Values are plain bytes of
     * the given size and alignment. `drop`, if not NULL, is called on a value before it is
     * discarded. Returns 0 on failure. */
    VoxlComponent (*component_register)(VoxlApp *app, const char *name, size_t name_len,
                                        size_t size, size_t align, void (*drop)(void *value));

    /* Finds a component defined by the engine or another plugin, e.g. "voxl.Transform".
     * Writes its size and alignment if the pointers are not NULL. Returns 0 if unknown. */
    VoxlComponent (*component_lookup)(VoxlApp *app, const char *name, size_t name_len,
                                      size_t *size, size_t *align);

    /* A zero-initialized block that lives as long as the app and survives reloads. Asking
     * again with the same name returns the same block (a fresh one if the size changed). */
    void *(*state)(VoxlApp *app, const char *name, size_t name_len, size_t size, size_t align);

    /* Adds a system. Returns 0 on success. */
    int32_t (*system_add)(VoxlApp *app, const VoxlSystemDesc *desc);

    /* ---- inside a system ---- */

    float (*delta_seconds)(VoxlSystem *system);    /* frame time, or the fixed timestep */
    double (*elapsed_seconds)(VoxlSystem *system); /* since the app started */

    /* Advances to the next matching entity. Writes one pointer per READ/WRITE term into
     * `components`. Returns 0 when there are no more. */
    uint8_t (*query_next)(VoxlSystem *system, VoxlEntity *entity, void **components);

    /* Looks up one entity directly. Returns 0 if it does not match the system's terms. */
    uint8_t (*query_get)(VoxlSystem *system, VoxlEntity entity, void **components);

    /* These take effect when the system returns. */
    VoxlEntity (*spawn)(VoxlSystem *system);
    void (*despawn)(VoxlSystem *system, VoxlEntity entity);
    /* Copies `value` (the component's size in bytes) onto the entity, replacing any old one. */
    void (*insert)(VoxlSystem *system, VoxlEntity entity, VoxlComponent component,
                   const void *value);
    void (*remove)(VoxlSystem *system, VoxlEntity entity, VoxlComponent component);

    /* ---- more queries (inside voxl_plugin_load, after the system is added) ---- */

    /* Gives a system another query, besides the one in its description (which is query 0).
     * Returns the new query's number (1, 2, ...) or a negative number on failure. Two queries
     * in one system may not both reach the same component unless one of them only reads, or
     * WITH/WITHOUT terms guarantee they never match the same entity. */
    int32_t (*system_add_query)(VoxlApp *app, const char *system, size_t system_len,
                                const VoxlTerm *terms, size_t term_count);

    /* ---- inside a system ---- */

    /* As query_next and query_get, for a numbered query. Each query keeps its own place. */
    uint8_t (*query_next_in)(VoxlSystem *system, uint32_t query, VoxlEntity *entity,
                             void **components);
    uint8_t (*query_get_in)(VoxlSystem *system, uint32_t query, VoxlEntity entity,
                            void **components);
    /* Starts a query again from its first entity. */
    void (*query_rewind)(VoxlSystem *system, uint32_t query);

    /* Input. "down" is held now; "pressed" and "released" are true only on the frame the key
     * or button changed. */
    uint8_t (*key_down)(VoxlSystem *system, uint32_t key);
    uint8_t (*key_pressed)(VoxlSystem *system, uint32_t key);
    uint8_t (*key_released)(VoxlSystem *system, uint32_t key);
    uint8_t (*mouse_down)(VoxlSystem *system, uint32_t button);
    uint8_t (*mouse_pressed)(VoxlSystem *system, uint32_t button);
    /* Writes how far the mouse moved this frame, in pixels, as x then y. */
    void (*mouse_motion)(VoxlSystem *system, float *delta);

    /* Meshes. Create one once (in a STARTUP system, keeping the handle in state) and share it
     * between entities. Triangles wind counter-clockwise seen from the front. Returns 0 on
     * failure. */
    VoxlMesh (*mesh_shape)(VoxlSystem *system, uint32_t shape, float a);
    VoxlMesh (*mesh_create)(VoxlSystem *system, const VoxlVertex *vertices, size_t vertex_count,
                            const uint32_t *indices, size_t index_count);

    /* Makes an entity visible: what it is drawn as, and with what surface. Like the other
     * changes, these take effect when the system returns. The entity also needs a
     * voxl.Transform. */
    void (*set_mesh)(VoxlSystem *system, VoxlEntity entity, VoxlMesh mesh);
    void (*set_material)(VoxlSystem *system, VoxlEntity entity, const VoxlMaterial *material);

    /* ---- events (register inside voxl_plugin_load) ---- */

    /* Defines an event type, or finds the one this name already has: `size` bytes each. Any
     * plugin that knows the name can send and read it; that is how plugins talk to each
     * other. The engine's own events are found the same way ("voxl.Contact"). Returns 0 on
     * failure, including the name existing with a different size. */
    VoxlEvent (*event_register)(VoxlApp *app, const char *name, size_t name_len, size_t size);

    /* ---- inside a system ---- */

    /* Sends an event, copying `size` bytes from `value`. Systems that run later this frame,
     * and every system next frame, can read it. */
    void (*event_send)(VoxlSystem *system, VoxlEvent event, const void *value);
    /* Copies the next event this system has not yet read into `value`. Returns 0 when there
     * are no more. Each system reads each event once. */
    uint8_t (*event_next)(VoxlSystem *system, VoxlEvent event, void *value);

    /* The scene. Like set_mesh, these take effect when the system returns, and the entity
     * also needs a voxl.Transform. */
    void (*set_camera)(VoxlSystem *system, VoxlEntity entity, const VoxlCamera *camera);
    void (*set_light)(VoxlSystem *system, VoxlEntity entity, const VoxlLight *light);
    /* Light arriving from every direction: an RGB colour and a strength. */
    void (*set_ambient)(VoxlSystem *system, const float *color, float intensity);
    void (*set_window_title)(VoxlSystem *system, const char *title, size_t len);

    /* Physics. These do nothing, and the queries find nothing, in an app without physics. */
    void (*set_collider)(VoxlSystem *system, VoxlEntity entity, const VoxlCollider *collider);
    void (*set_body)(VoxlSystem *system, VoxlEntity entity, const VoxlBody *body);
    /* A sudden push on a dynamic body: three floats, in newton-seconds. */
    void (*apply_impulse)(VoxlSystem *system, VoxlEntity entity, const float *impulse);
    void (*set_velocity)(VoxlSystem *system, VoxlEntity entity, const float *velocity);
    /* Writes the body's velocity as three floats. Returns 0 if the entity has no body. */
    uint8_t (*velocity)(VoxlSystem *system, VoxlEntity entity, float *velocity);
    /* The first collider a ray meets within `max_distance`, as things stood after the last
     * physics step. `direction` need not be normalized. Returns 0 if it meets nothing. */
    uint8_t (*raycast)(VoxlSystem *system, const float *origin, const float *direction,
                       float max_distance, VoxlRayHit *hit);

    /* ---- describing components (inside voxl_plugin_load) ---- */

    /* Says what fields a component this plugin registered has. From then on the engine can
     * save it in scenes, load it back, and show it in an inspector, like its own components.
     * Bytes that no field covers are not saved, and load as zero. Returns 0 on success. */
    int32_t (*component_describe)(VoxlApp *app, VoxlComponent component, const VoxlField *fields,
                                  size_t field_count);

    /* ---- files (inside a system) ---- */

    /* Loads an image by name: a PNG or JPEG path relative to the app's asset folder, with
     * "?linear" appended for data such as normal maps. The same name always gives the same
     * handle. The image arrives a moment later; until then it draws as plain white. */
    VoxlImage (*image_load)(VoxlSystem *system, const char *name, size_t len);
    /* Sets the textures of an entity's material (giving it a default material if it has
     * none). Pass 0 for a texture to leave that one unset. */
    void (*set_textures)(VoxlSystem *system, VoxlEntity entity, VoxlImage base_color,
                         VoxlImage normal, VoxlImage metallic_roughness);
    /* Spawns a glTF model (.gltf or .glb) by name: one new entity at `transform`, with a
     * child entity for each part of the model. Returns the new entity at once; the parts
     * appear when the system returns. */
    VoxlEntity (*spawn_model)(VoxlSystem *system, const char *name, size_t len,
                              const VoxlTransform *transform);

    /* ---- hierarchy (inside a system; these take effect when it returns) ---- */

    /* Makes `child` a child of `parent`: its transform becomes relative to the parent's.
     * Pass VOXL_ENTITY_NONE as the parent to make it a root again. */
    void (*set_parent)(VoxlSystem *system, VoxlEntity child, VoxlEntity parent);
    /* Despawns an entity and everything below it (a model and its parts, say). */
    void (*despawn_tree)(VoxlSystem *system, VoxlEntity entity);

    /* ---- prefabs (inside a system) ---- */

    /* Spawns an instance of a prefab by name (a scene file relative to the app's asset
     * folder, or a name the app gave a scene): one new entity at `transform`, with the
     * prefab's entities below it. Returns the new entity at once; its contents appear on
     * the next frame, and are rebuilt whenever the prefab's file is saved again. */
    VoxlEntity (*spawn_prefab)(VoxlSystem *system, const char *name, size_t len,
                               const VoxlTransform *transform);

    /* ---- failures (inside a system) ---- */

    /* Says that this run of the system has failed: an exception was thrown, an assertion
     * did not hold. `trace` is whatever the language can say about where (a stack, a source
     * location), and may be empty. Call it and return. In a debug build the engine then
     * pauses the game with the message and trace on show and leaves the system out until the
     * plugin is reloaded; otherwise it logs them. What the system queued this run (spawns,
     * inserts) is dropped. Language bindings call this for you when a system throws. */
    void (*system_fail)(VoxlSystem *system, const char *message, size_t len, const char *trace,
                        size_t trace_len);

    /* ---- signals (inside a system) ----
     * Values derived from the world and from each other, which the engine keeps true; see
     * docs/SIGNALS.md. A plugin gives the graph its own facts by setting signals from a
     * system, builds rules on them as data, and reads any signal by name. */

    /* Sets a signal to a truth (`number` 0: any `value` but 0 is true) or to a number
     * (`number` 1), defining it if need be. Takes effect when the system returns. */
    void (*signal_set)(VoxlSystem *system, const char *name, size_t len, double value,
                       uint32_t number);
    /* Reads a signal as of its last update: returns 0 if there is none, else 1 and writes
     * its value to `out` (a truth as 1 or 0). */
    uint32_t (*signal_get)(VoxlSystem *system, const char *name, size_t len, double *out);
    /* Defines a signal worked out from others, or replaces its definition, when the system
     * returns. `op` is a VOXL_SIGNAL_ value; `inputs` is the input signals' names separated
     * by spaces; `param` is the seconds of VOXL_SIGNAL_HELD_FOR, and ignored otherwise. */
    void (*signal_define)(VoxlSystem *system, const char *name, size_t len, uint32_t op,
                          double param, const char *inputs, size_t inputs_len);
} VoxlApi;

/* ---- conveniences for C and C++ ---- */

/* Expands a string literal to the (pointer, length) pair the API takes. */
#define VOXL_STR(literal) (literal), (sizeof(literal) - 1)

#if defined(_WIN32)
#define VOXL_EXPORT __declspec(dllexport)
#else
#define VOXL_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
}
#endif

#endif /* VOXL_H */
