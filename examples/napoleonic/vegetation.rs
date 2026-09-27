//! Trees and shrubs: EZ-Tree models (MIT, bark from Poly Haven, CC0) and a Poly Haven shrub
//! scan, each with a simplified middle level and a far impostor made by `tools/trees.py`.

use voxl::{
    glam::Vec3,
    prelude::*,
    render::{GltfScene, Image},
};

use crate::materials::asset;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Species {
    Oak,
    Ash,
    Poplar,
    Shrub,
}

impl Species {
    fn file(self) -> &'static str {
        match self {
            Species::Oak => "oak",
            Species::Ash => "ash",
            Species::Poplar => "poplar",
            Species::Shrub => "bush",
        }
    }

    /// Where each level stops being drawn.
    fn distances(self) -> [f32; 3] {
        match self {
            Species::Shrub => [30.0, 90.0, 900.0],
            _ => [45.0, 170.0, 5000.0],
        }
    }
}

/// Each species' levels of detail, as (level, part) → mesh and material.
pub struct Forest {
    species: Vec<(Species, Vec<Vec<LodLevel>>)>,
}

fn tweak(name: &str, m: &mut Material) {
    let leafy = m.alpha_cutoff.is_some();
    if leafy {
        // EZ-Tree's leaf texture is brighter and more saturated than summer oak and ash.
        m.color = Color::rgb(m.color.r * 0.78, m.color.g * 0.8, m.color.b * 0.62);
        m.translucency = 0.45;
        m.roughness = m.roughness.max(0.6);
        m.double_sided = true;
    } else {
        m.roughness = 1.0;
    }
    let _ = name;
}

impl Forest {
    pub fn load(meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> anyhow::Result<Self> {
        let mut species = Vec::new();
        for kind in [Species::Oak, Species::Ash, Species::Poplar, Species::Shrub] {
            let distances = kind.distances();
            let levels: Vec<GltfScene> = (0..3)
                .map(|lod| {
                    let path = asset(&format!("models/trees/{}_lod{lod}.glb", kind.file()));
                    GltfScene::load(path, meshes, images).map(|s| s.with_materials(tweak))
                })
                .collect::<anyhow::Result<_>>()?;
            // A level of detail has no transform of its own, so bake the parts' in.
            let mut levels = levels;
            for level in &mut levels {
                for part in &mut level.parts {
                    if part.transform != Mat4::IDENTITY {
                        let mut baked = Mesh::default();
                        if let Some(mesh) = meshes.get(part.mesh) {
                            baked.append(mesh, part.transform);
                        }
                        part.mesh = meshes.add(baked);
                        part.transform = Mat4::IDENTITY;
                    }
                }
            }
            // Parts are matched up by order across the levels: bark with the front card,
            // leaves with the side card.
            let parts = levels.iter().map(|l| l.parts.len()).max().unwrap_or(0);
            let per_part = (0..parts)
                .map(|p| {
                    levels
                        .iter()
                        .zip(distances)
                        .filter_map(|(level, max_distance)| {
                            level.parts.get(p).map(|part| LodLevel {
                                max_distance,
                                mesh: part.mesh,
                                material: part.material,
                            })
                        })
                        .collect()
                })
                .collect();
            species.push((kind, per_part));
        }
        Ok(Self { species })
    }

    pub fn spawn(&self, world: &mut World, kind: Species, at: Vec3, yaw: f32, scale: f32) {
        let Some((_, parts)) = self.species.iter().find(|(k, _)| *k == kind) else {
            return;
        };
        let transform = Transform::from_translation(at)
            .with_rotation(Quat::from_rotation_y(yaw))
            .with_scale(Vec3::splat(scale));
        for levels in parts {
            world.spawn((transform, Lods(levels.clone())));
        }
    }
}
