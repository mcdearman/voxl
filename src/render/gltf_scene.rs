//! Loads glTF 2.0 scenes (`.gltf` + `.bin`, or `.glb`) as meshes and PBR materials.
//!
//! Skins and animations are ignored: skinned meshes load in their bind pose, so pose figures
//! in a modelling tool and export the result.

use std::{collections::HashMap, path::Path};

use anyhow::Context;
use glam::{Mat4, Vec2, Vec3};

use super::{
    animation::{AnimationClip, Animator, Channel, Property, SkinWeights, Skeleton, Skinned},
    image::Image,
    mesh::Vertex,
    Color, Material, Mesh, Mesh3d,
};
use crate::{
    assets::{Assets, Handle},
    ecs::{Commands, Entity, World},
    transform::{Parent, Transform},
};

/// One drawable piece of a glTF scene: a primitive, its material, and where it sits relative
/// to the scene's origin.
#[derive(Clone, Debug)]
pub struct GltfPart {
    pub name: String,
    pub mesh: Handle<Mesh>,
    pub material: Material,
    /// The material's name in the file, for recolouring particular parts.
    pub material_name: String,
    pub transform: Mat4,
    /// For a skinned part, the joints moving each vertex. Its transform is then the identity:
    /// the skeleton places it.
    pub skin: Option<std::sync::Arc<SkinWeights>>,
}

/// A loaded glTF file.
#[derive(Clone, Debug, Default)]
pub struct GltfScene {
    pub parts: Vec<GltfPart>,
    /// Bounds of all parts, in scene space.
    pub min: Vec3,
    pub max: Vec3,
    /// The skeleton skinned parts bind to, and the file's animations, if it has them.
    pub skeleton: Option<std::sync::Arc<Skeleton>>,
    pub clips: Vec<std::sync::Arc<AnimationClip>>,
}

impl GltfScene {
    /// Loads the default scene. Meshes and images go into the asset stores; the returned parts
    /// refer to them, so spawning many copies shares everything.
    pub fn load(path: impl AsRef<Path>, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let (document, buffers, image_data) =
            gltf::import(path).with_context(|| format!("loading {}", path.display()))?;

        // An image may be colour in one material and data in another; upload each use once.
        // Files often repeat each other's images (a figure in several poses, say), so images
        // are also shared across files by their content.
        let mut uploaded: HashMap<(usize, bool), Handle<Image>> = HashMap::new();
        let mut image = |index: usize, srgb: bool| -> Handle<Image> {
            *uploaded.entry((index, srgb)).or_insert_with(|| {
                let data = &image_data[index];
                let key = (content_hash(&data.pixels, data.width, data.height), srgb);
                let mut shared = SHARED_IMAGES.lock().unwrap_or_else(|e| e.into_inner());
                *shared.get_or_insert_with(HashMap::new).entry(key).or_insert_with(|| images.add(to_image(data, srgb)))
            })
        };
        let materials: Vec<Material> = document
            .materials()
            .map(|m| {
                let pbr = m.pbr_metallic_roughness();
                let [r, g, b, a] = pbr.base_color_factor();
                let [er, eg, eb] = m.emissive_factor();
                let strength = m.emissive_strength().unwrap_or(1.0);
                Material {
                    color: Color { r, g, b, a },
                    roughness: pbr.roughness_factor(),
                    metallic: pbr.metallic_factor(),
                    emissive: Color::rgb(er * strength, eg * strength, eb * strength),
                    translucency: 0.0,
                    base_color_texture: pbr.base_color_texture().map(|t| image(t.texture().source().index(), true)),
                    normal_texture: m.normal_texture().map(|t| image(t.texture().source().index(), false)),
                    normal_strength: m.normal_texture().map_or(1.0, |t| t.scale()),
                    metallic_roughness_texture: pbr
                        .metallic_roughness_texture()
                        .map(|t| image(t.texture().source().index(), false)),
                    // Blended glTF materials are cut out instead: close enough for foliage and
                    // hair, and no sorting needed.
                    alpha_cutoff: match m.alpha_mode() {
                        gltf::material::AlphaMode::Opaque => None,
                        gltf::material::AlphaMode::Mask => Some(m.alpha_cutoff().unwrap_or(0.5)),
                        gltf::material::AlphaMode::Blend => Some(0.5),
                    },
                    double_sided: m.double_sided(),
                    ..Default::default()
                }
            })
            .collect();

        let mut scene = GltfScene {
            parts: Vec::new(),
            min: Vec3::splat(f32::MAX),
            max: Vec3::splat(f32::MIN),
            skeleton: read_skeleton(&document, &buffers).map(std::sync::Arc::new),
            clips: read_clips(&document, &buffers).into_iter().map(std::sync::Arc::new).collect(),
        };
        let root = document
            .default_scene()
            .or_else(|| document.scenes().next())
            .context("the file has no scenes")?;
        let mut stack: Vec<(gltf::Node, Mat4)> = root.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        let mut loaded: HashMap<(usize, usize), Handle<Mesh>> = HashMap::new();
        while let Some((node, parent)) = stack.pop() {
            let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            if let Some(mesh) = node.mesh() {
                let skinned = node.skin().is_some() && scene.skeleton.is_some();
                for primitive in mesh.primitives() {
                    let key = (mesh.index(), primitive.index());
                    let handle = match loaded.get(&key) {
                        Some(h) => *h,
                        None => {
                            let Some(m) = read_primitive(&primitive, &buffers)? else {
                                continue;
                            };
                            let h = meshes.add(m);
                            loaded.insert(key, h);
                            h
                        }
                    };
                    if let Some(m) = meshes.get(handle) {
                        for v in &m.vertices {
                            let p = transform.transform_point3(Vec3::from(v.position));
                            scene.min = scene.min.min(p);
                            scene.max = scene.max.max(p);
                        }
                    }
                    let material = primitive
                        .material()
                        .index()
                        .map_or_else(Material::default, |i| materials[i]);
                    let material_name = primitive.material().name().unwrap_or_default().to_string();
                    let skin = if skinned { read_weights(&primitive, &buffers).map(std::sync::Arc::new) } else { None };
                    scene.parts.push(GltfPart {
                        name: node.name().or(mesh.name()).unwrap_or_default().to_string(),
                        mesh: handle,
                        material,
                        material_name,
                        // A skinned mesh ignores its node's transform; its joints place it.
                        transform: if skin.is_some() { Mat4::IDENTITY } else { transform },
                        skin,
                    });
                }
            }
            stack.extend(node.children().map(|c| (c, transform)));
        }
        Ok(scene)
    }

    /// Spawns every part as a child of a new entity placed at `transform`, and returns it.
    pub fn spawn(&self, commands: &mut Commands, transform: Transform) -> Entity {
        let root = commands.spawn(transform).id();
        for part in &self.parts {
            let (scale, rotation, translation) = part.transform.to_scale_rotation_translation();
            commands.spawn((
                Transform {
                    translation,
                    rotation,
                    scale,
                },
                Mesh3d(part.mesh),
                part.material,
                Parent(root),
            ));
        }
        root
    }

    /// Spawns a copy that can be animated, and returns its root, which carries an `Animator`
    /// playing the file's clips. Each skinned part gets its own copy of its mesh, which the GPU
    /// poses every frame. `material` picks each part's material.
    pub fn spawn_animated(&self, world: &mut World, transform: Transform, material: impl Fn(&GltfPart) -> Material) -> Entity {
        let root = match &self.skeleton {
            Some(skeleton) => world.spawn((transform, Animator::new(skeleton.clone(), self.clips.clone()))),
            None => world.spawn(transform),
        };
        let palette = world.get::<Animator>(root).map(|a| a.palette.clone());
        for part in &self.parts {
            let m = material(part);
            match (&part.skin, &palette) {
                (Some(weights), Some(palette)) => {
                    let meshes = world.resource_mut::<Assets<Mesh>>();
                    let Some(copy) = meshes.get(part.mesh).cloned() else { continue };
                    let mesh = meshes.add(copy);
                    let skinned = Skinned {
                        source: part.mesh,
                        weights: weights.clone(),
                        palette: palette.clone(),
                    };
                    world.spawn((Transform::IDENTITY, Mesh3d(mesh), m, skinned, Parent(root)));
                }
                _ => {
                    let (scale, rotation, translation) = part.transform.to_scale_rotation_translation();
                    world.spawn((Transform { translation, rotation, scale }, Mesh3d(part.mesh), m, Parent(root)));
                }
            }
        }
        root
    }

    /// Every part with `f` applied to its material, for recolouring or tweaking a model.
    pub fn with_materials(mut self, f: impl Fn(&str, &mut Material)) -> Self {
        for part in &mut self.parts {
            f(&part.name, &mut part.material);
        }
        self
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// Every node of the file as a joint hierarchy, with the first skin's joints.
fn read_skeleton(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Option<Skeleton> {
    let skin = document.skins().next()?;
    let count = document.nodes().len();
    let mut parents = vec![None; count];
    for node in document.nodes() {
        for child in node.children() {
            parents[child.index()] = Some(node.index());
        }
    }
    let names = document.nodes().map(|n| n.name().unwrap_or_default().to_string()).collect();
    let rest = document
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            (Vec3::from(t), glam::Quat::from_array(r), Vec3::from(s))
        })
        .collect();
    // Parents before children: walk down from the roots.
    let mut order = Vec::with_capacity(count);
    let mut stack: Vec<usize> = (0..count).filter(|&i| parents[i].is_none()).collect();
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(document.nodes().nth(i).into_iter().flat_map(|n| n.children().map(|c| c.index())));
    }
    let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    let reader = skin.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let inverse_bind = match reader.read_inverse_bind_matrices() {
        Some(m) => m.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
        None => vec![Mat4::IDENTITY; joints.len()],
    };
    Some(Skeleton {
        names,
        parents,
        rest,
        order,
        joints,
        inverse_bind,
    })
}

fn read_clips(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<AnimationClip> {
    use gltf::animation::{util::ReadOutputs, Interpolation};
    document
        .animations()
        .map(|animation| {
            let mut duration: f32 = 0.0;
            let mut channels = Vec::new();
            for channel in animation.channels() {
                let reader = channel.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(times) = reader.read_inputs() else { continue };
                let times: Vec<f32> = times.collect();
                let (property, values): (Property, Vec<[f32; 4]>) = match reader.read_outputs() {
                    Some(ReadOutputs::Translations(t)) => (Property::Translation, t.map(|v| [v[0], v[1], v[2], 0.0]).collect()),
                    Some(ReadOutputs::Scales(s)) => (Property::Scale, s.map(|v| [v[0], v[1], v[2], 0.0]).collect()),
                    Some(ReadOutputs::Rotations(r)) => (Property::Rotation, r.into_f32().collect()),
                    _ => continue,
                };
                let interpolation = channel.sampler().interpolation();
                // Cubic splines store (in-tangent, value, out-tangent); keep the values.
                let values = if interpolation == Interpolation::CubicSpline {
                    values.chunks(3).filter_map(|c| c.get(1).copied()).collect()
                } else {
                    values
                };
                if times.is_empty() || values.len() < times.len() {
                    continue;
                }
                duration = duration.max(*times.last().unwrap());
                channels.push(Channel {
                    node: channel.target().node().index(),
                    property,
                    times,
                    values,
                    step: interpolation == Interpolation::Step,
                });
            }
            AnimationClip {
                name: animation.name().unwrap_or_default().to_string(),
                duration,
                channels,
            }
        })
        .collect()
}

fn read_weights(primitive: &gltf::Primitive, buffers: &[gltf::buffer::Data]) -> Option<SkinWeights> {
    let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let joints: Vec<[u16; 4]> = reader.read_joints(0)?.into_u16().collect();
    let weights: Vec<[f32; 4]> = reader.read_weights(0)?.into_f32().collect();
    Some(SkinWeights { joints, weights })
}

fn read_primitive(primitive: &gltf::Primitive, buffers: &[gltf::buffer::Data]) -> anyhow::Result<Option<Mesh>> {
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        return Ok(None);
    }
    let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let Some(positions) = reader.read_positions() else {
        return Ok(None);
    };
    let positions: Vec<Vec3> = positions.map(Vec3::from).collect();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    let normals: Vec<Vec3> = match reader.read_normals() {
        Some(n) => n.map(Vec3::from).collect(),
        None => flat_normals(&positions, &indices),
    };
    let uvs: Vec<Vec2> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().map(Vec2::from).collect())
        .unwrap_or_else(|| vec![Vec2::ZERO; positions.len()]);
    let colors: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
    let vertices = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut v = Vertex::new(*p, normals[i], uvs[i]);
            if let Some(c) = &colors {
                v.color = [c[i][0], c[i][1], c[i][2]];
            }
            v
        })
        .collect();
    Ok(Some(Mesh { vertices, indices }))
}

/// Per-vertex normals averaged from the faces, for files that leave them out.
fn flat_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
        let n = (b - a).cross(c - a);
        for i in t {
            normals[*i as usize] += n;
        }
    }
    normals.iter().map(|n| n.normalize_or(Vec3::Y)).collect()
}

fn to_image(data: &gltf::image::Data, srgb: bool) -> Image {
    use gltf::image::Format;
    let pixels = data.width as usize * data.height as usize;
    let rgba: Vec<u8> = match data.format {
        Format::R8G8B8A8 => data.pixels.clone(),
        Format::R8G8B8 => data.pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        Format::R8G8 => data.pixels.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[1], 0, 255]).collect(),
        Format::R8 => data.pixels.iter().flat_map(|p| [*p, *p, *p, 255]).collect(),
        Format::R16G16B16A16 => data.pixels.as_chunks::<8>().0.iter().flat_map(|p| [p[1], p[3], p[5], p[7]]).collect(),
        Format::R16G16B16 => data.pixels.as_chunks::<6>().0.iter().flat_map(|p| [p[1], p[3], p[5], 255]).collect(),
        other => {
            log::warn!("unsupported glTF image format {other:?}; using white");
            vec![255; pixels * 4]
        }
    };
    Image::from_rgba(data.width, data.height, rgba, srgb)
}

/// Images loaded from any glTF file, by content, so identical ones are uploaded once.
type SharedImages = HashMap<(u64, bool), Handle<Image>>;
static SHARED_IMAGES: std::sync::Mutex<Option<SharedImages>> = std::sync::Mutex::new(None);

fn content_hash(pixels: &[u8], width: u32, height: u32) -> u64 {
    // FNV-style over whole words: quick enough to read every byte of a big texture.
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ ((width as u64) << 32 | height as u64);
    let (words, tail) = pixels.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x0100_0000_01b3);
    }
    for b in tail {
        h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}
