"""Builds the levels of detail for a tree model.

    blender -b --python trees.py -- <input.gltf> <output dir> <name>

Writes <name>_lod0.glb (the original), <name>_lod1.glb (fewer, larger leaf cards and a
simplified trunk) and <name>_lod2.glb (an impostor: two crossed cards showing renders of the
tree, for the far distance).

The tree needs a mesh whose material clips alpha (the leaves) and opaque meshes (the bark).
"""

import math
import os
import random
import sys
import tempfile

import bmesh
import bpy
from mathutils import Vector

src, out_dir, name = sys.argv[-3:]
os.makedirs(out_dir, exist_ok=True)
random.seed(7)


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=src)
    return [o for o in bpy.data.objects if o.type == "MESH"]


def export(path, objects):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objects:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_yup=True,
        export_image_format="AUTO",
    )


def is_leaves(o):
    return any(m and m.blend_method in {"CLIP", "BLEND", "HASHED"} for m in o.data.materials)


def shade_canopy(objects):
    """Makes a crown of leaf cards light like a crown rather than a pile of paper.

    Normals are bent outward from the crown's centre, so the sunward side is bright and the far
    side dim, as with a real tree; and the crown's depth is baked into vertex colours as
    occlusion, so its heart and underside are dark, where little skylight reaches."""
    leaves = [o for o in objects if is_leaves(o)]
    if not leaves:
        return
    pts = [o.matrix_world @ v.co for o in leaves for v in o.data.vertices]
    lo = Vector([min(p[i] for p in pts) for i in range(3)])
    hi = Vector([max(p[i] for p in pts) for i in range(3)])
    center = (lo + hi) / 2
    half = (hi - lo) / 2
    half = Vector([max(h, 0.05) for h in half])
    for o in leaves:
        me = o.data
        inv = o.matrix_world.inverted()
        if not me.color_attributes:
            me.color_attributes.new("Col", "FLOAT_COLOR", "POINT")
        col = me.color_attributes[0]
        normals = []
        for v in me.vertices:
            p = o.matrix_world @ v.co
            rel = Vector(((p.x - center.x) / half.x, (p.y - center.y) / half.y, (p.z - center.z) / half.z))
            depth = min(rel.length, 1.0)
            height = (p.z - lo.z) / (hi.z - lo.z)
            # Outer leaves see the sky; inner and lower ones are shaded by the rest.
            t = max(0.0, min(1.0, (depth - 0.25) / 0.75))
            ao = (0.25 + 0.75 * t * t * (3 - 2 * t)) * (0.7 + 0.3 * height)
            col.data[v.index].color = (ao, ao, ao, 1.0)
            outward = (p - center).normalized()
            card = (o.matrix_world.to_3x3() @ v.normal).normalized()
            if card.dot(outward) < 0:
                card = -card
            normals.append((inv.to_3x3() @ (card * 0.35 + outward).normalized()).normalized())
        me.use_auto_smooth = True
        me.normals_split_custom_set_from_vertices(normals)


# ---- LOD 0: as it came, with the crown shaded.
objects = reset()
shade_canopy(objects)
export(os.path.join(out_dir, f"{name}_lod0.glb"), objects)

# ---- LOD 1: keep a third of the leaf cards, grown to cover the gaps; decimate the wood.
objects = reset()
for o in objects:
    bpy.context.view_layer.objects.active = o
    if is_leaves(o):
        bm = bmesh.new()
        bm.from_mesh(o.data)
        islands = []
        seen = set()
        for f in bm.faces:
            if f.index in seen:
                continue
            stack, island = [f], []
            seen.add(f.index)
            while stack:
                face = stack.pop()
                island.append(face)
                for e in face.edges:
                    for linked in e.link_faces:
                        if linked.index not in seen:
                            seen.add(linked.index)
                            stack.append(linked)
            islands.append(island)
        doomed = []
        for island in islands:
            if random.random() < 0.3:
                verts = {v for f in island for v in f.verts}
                c = sum((v.co for v in verts), Vector()) / len(verts)
                for v in verts:
                    v.co = c + (v.co - c) * 1.65
            else:
                doomed.extend(island)
        bmesh.ops.delete(bm, geom=doomed, context="FACES")
        bm.to_mesh(o.data)
        bm.free()
    else:
        m = o.modifiers.new("decimate", "DECIMATE")
        m.ratio = 0.25
shade_canopy(objects)
export(os.path.join(out_dir, f"{name}_lod1.glb"), objects)

# ---- LOD 2: render the tree's colour, unlit, from the front and the side.
objects = reset()
lo = Vector((min(v[i] for o in objects for v in [o.matrix_world @ Vector(c) for c in o.bound_box]) for i in range(3)))
hi = Vector((max(v[i] for o in objects for v in [o.matrix_world @ Vector(c) for c in o.bound_box]) for i in range(3)))
size = hi - lo
center = (lo + hi) / 2

# Swap every material's shading for its plain base colour, keeping alpha for the cutout.
for mat in bpy.data.materials:
    if not mat.use_nodes:
        continue
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    bsdf = next((n for n in nodes if n.type == "BSDF_PRINCIPLED"), None)
    out = next((n for n in nodes if n.type == "OUTPUT_MATERIAL"), None)
    if not bsdf or not out:
        continue
    emit = nodes.new("ShaderNodeEmission")
    mix = nodes.new("ShaderNodeMixShader")
    clear = nodes.new("ShaderNodeBsdfTransparent")
    base = bsdf.inputs["Base Color"]
    if base.is_linked:
        links.new(base.links[0].from_socket, emit.inputs["Color"])
    else:
        emit.inputs["Color"].default_value = base.default_value
    alpha = bsdf.inputs["Alpha"]
    if alpha.is_linked:
        # Hard cutout, as the engine draws it.
        ramp = nodes.new("ShaderNodeMath")
        ramp.operation = "GREATER_THAN"
        ramp.inputs[1].default_value = 0.5
        links.new(alpha.links[0].from_socket, ramp.inputs[0])
        links.new(ramp.outputs[0], mix.inputs["Fac"])
    else:
        mix.inputs["Fac"].default_value = 1.0
    links.new(clear.outputs[0], mix.inputs[1])
    links.new(emit.outputs[0], mix.inputs[2])
    links.new(mix.outputs[0], out.inputs["Surface"])
    mat.blend_method = "CLIP"

scene = bpy.context.scene
scene.render.engine = "CYCLES"
scene.cycles.samples = 16
scene.cycles.device = "CPU"
scene.render.film_transparent = True
scene.view_settings.view_transform = "Standard"
width_m = max(size.x, size.y)
height_m = size.z
res_x = 512
res_y = int(res_x * height_m / width_m / 8) * 8 or 8
scene.render.resolution_x = res_x
scene.render.resolution_y = max(res_y, 64)
scene.render.image_settings.file_format = "PNG"
scene.render.image_settings.color_mode = "RGBA"

cam_data = bpy.data.cameras.new("cam")
cam_data.type = "ORTHO"
cam_data.ortho_scale = max(width_m, height_m) * 1.02
cam = bpy.data.objects.new("cam", cam_data)
scene.collection.objects.link(cam)
scene.camera = cam
scene.world = bpy.data.worlds.new("w")

renders = []
for i, angle in enumerate([0.0, math.pi / 2]):
    d = Vector((math.sin(angle), -math.cos(angle), 0.0))
    cam.location = center + d * 100
    cam.rotation_euler = (math.pi / 2, 0, angle)
    path = os.path.join(tempfile.gettempdir(), f"{name}_impostor_{i}.png")
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    renders.append(path)

# The cards: two quads crossing at the trunk, each with its render.
bpy.ops.wm.read_factory_settings(use_empty=True)
half_w = max(size.x, size.y) * 1.02 / 2
bottom = lo.z
top = bottom + height_m * 1.02
cards = []
for i, angle in enumerate([0.0, math.pi / 2]):
    mesh = bpy.data.meshes.new(f"card{i}")
    right = Vector((math.cos(angle), math.sin(angle), 0.0))
    cx, cy = center.x, center.y
    verts = [
        Vector((cx, cy, bottom)) - right * half_w,
        Vector((cx, cy, bottom)) + right * half_w,
        Vector((cx, cy, top)) + right * half_w,
        Vector((cx, cy, top)) - right * half_w,
    ]
    mesh.from_pydata([tuple(v) for v in verts], [], [(0, 1, 2, 3)])
    uv = mesh.uv_layers.new()
    for loop, coord in zip(uv.data, [(0, 0), (1, 0), (1, 1), (0, 1)]):
        loop.uv = coord
    # Normals bulge outward like a rounded crown, so sunlight still models the tree.
    mesh.use_auto_smooth = True
    crown = Vector((cx, cy, bottom + height_m * 0.6))
    normals = []
    for v in verts:
        n = (v - crown)
        n.z = max(n.z, 0.0) + height_m * 0.25
        normals.append(n.normalized())
    mesh.normals_split_custom_set_from_vertices(normals)
    img = bpy.data.images.load(renders[i])
    mat = bpy.data.materials.new(f"impostor{i}")
    mat.use_nodes = True
    mat.blend_method = "CLIP"
    mat.use_backface_culling = False
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    tex = mat.node_tree.nodes.new("ShaderNodeTexImage")
    tex.image = img
    mat.node_tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    mat.node_tree.links.new(tex.outputs["Alpha"], bsdf.inputs["Alpha"])
    bsdf.inputs["Roughness"].default_value = 0.9
    bsdf.inputs["Specular"].default_value = 0.2
    mesh.materials.append(mat)
    obj = bpy.data.objects.new(f"card{i}", mesh)
    bpy.context.scene.collection.objects.link(obj)
    cards.append(obj)
export(os.path.join(out_dir, f"{name}_lod2.glb"), cards)
print("DONE", name, size)
