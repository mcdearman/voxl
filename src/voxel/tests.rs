use glam::{IVec3, Vec3};

use super::{mesher::FACES, *};

fn registry() -> (BlockRegistry, BlockId) {
    let mut registry = BlockRegistry::default();
    let stone = registry.register(Block::uniform("stone", 3));
    (registry, stone)
}

/// A 3×3×3 block of empty chunks, so the middle one has every neighbour.
fn empty_world() -> VoxelWorld {
    let mut world = VoxelWorld::default();
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                world.insert_chunk(IVec3::new(x, y, z), ChunkData::default());
            }
        }
    }
    world
}

fn mesh(world: &VoxelWorld, registry: &BlockRegistry, chunk: IVec3) -> ChunkMesh {
    mesh_chunk(&PaddedChunk::capture(world, chunk), registry, chunk)
}

#[test]
fn chunk_data_stays_uniform_until_it_has_to_change() {
    let (_, stone) = registry();
    let mut data = ChunkData::default();
    data.set(IVec3::ONE, BlockId::AIR);
    assert!(data.is_uniform(BlockId::AIR));

    assert_eq!(data.set(IVec3::ONE, stone), BlockId::AIR);
    assert_eq!(data.get(IVec3::ONE), stone);
    assert_eq!(data.get(IVec3::ZERO), BlockId::AIR);

    data.set(IVec3::ONE, BlockId::AIR);
    data.compact();
    assert!(data.is_uniform(BlockId::AIR));
}

#[test]
fn negative_coordinates_map_to_the_right_chunk() {
    assert_eq!(chunk_of(IVec3::new(-1, 0, 32)), IVec3::new(-1, 0, 1));
    assert_eq!(local_of(IVec3::new(-1, 0, 32)), IVec3::new(31, 0, 0));

    let (_, stone) = registry();
    let mut world = empty_world();
    assert!(world.set_block(IVec3::new(-1, -32, 5), stone));
    assert_eq!(world.get_block(IVec3::new(-1, -32, 5)), Some(stone));
    assert_eq!(world.get_block(IVec3::new(500, 0, 0)), None);
    assert!(!world.set_block(IVec3::new(500, 0, 0), stone));
}

#[test]
fn edits_on_a_corner_dirty_every_chunk_that_sees_them() {
    let (_, stone) = registry();
    let mut world = empty_world();
    world.dirty.clear();
    world.set_block(IVec3::new(5, 5, 5), stone);
    assert_eq!(world.dirty.len(), 1);

    world.dirty.clear();
    world.set_block(IVec3::ZERO, stone);
    assert_eq!(world.dirty.len(), 8);
    assert!(world.dirty.contains(&IVec3::NEG_ONE));
}

#[test]
fn a_single_block_has_six_outward_faces() {
    let (registry, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(4, 5, 6), stone);
    let mesh = mesh(&world, &registry, IVec3::ZERO);
    assert_eq!(mesh.vertices.len(), 24);
    assert_eq!(mesh.indices.len(), 36);

    let center = Vec3::new(4.5, 5.5, 6.5);
    for tri in mesh.indices.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[tri[i] as usize]);
        let (pa, pb, pc) = (
            Vec3::from(a.position),
            Vec3::from(b.position),
            Vec3::from(c.position),
        );
        let winding = (pb - pa).cross(pc - pa).normalize();
        let expected = FACES[(a.packed & 7) as usize].0.as_vec3();
        assert!(
            winding.distance(expected) < 1e-5,
            "face winds the wrong way"
        );
        assert!(
            (pa - center).dot(expected) > 0.0,
            "face isn't on its own side"
        );
        assert_eq!(a.packed >> 3 & 3, 3, "a lone block has no occlusion");
        assert_eq!(a.packed >> 5, 3, "texture layer");
    }
}

#[test]
fn faces_between_blocks_are_hidden_even_across_chunks() {
    let (registry, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(0, 8, 8), stone);
    world.set_block(IVec3::new(-1, 8, 8), stone);
    // Five faces each: the shared one is gone from both chunks.
    assert_eq!(mesh(&world, &registry, IVec3::ZERO).indices.len(), 5 * 6);
    assert_eq!(
        mesh(&world, &registry, IVec3::new(-1, 0, 0)).indices.len(),
        5 * 6
    );
}

#[test]
fn unloaded_neighbours_hide_border_faces() {
    let (registry, stone) = registry();
    let mut world = VoxelWorld::default();
    world.insert_chunk(IVec3::ZERO, ChunkData::Uniform(stone));
    assert!(mesh(&world, &registry, IVec3::ZERO).indices.is_empty());
}

#[test]
fn corners_next_to_walls_are_occluded() {
    let (registry, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(8, 8, 8), stone);
    world.set_block(IVec3::new(9, 9, 8), stone); // a step up beside the top face
    let mesh = mesh(&world, &registry, IVec3::ZERO);
    let top_of_floor: Vec<u32> = mesh
        .vertices
        .iter()
        .filter(|v| v.packed & 7 == 2 && v.position[1] == 9.0 && v.position[0] <= 9.0)
        .map(|v| v.packed >> 3 & 3)
        .collect();
    assert_eq!(top_of_floor.len(), 4);
    assert_eq!(top_of_floor.iter().filter(|ao| **ao == 2).count(), 2);
    assert_eq!(top_of_floor.iter().filter(|ao| **ao == 3).count(), 2);
}

#[test]
fn raycast_reports_the_block_and_face() {
    let (registry, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(10, 3, 3), stone);

    let hit = world
        .raycast(&registry, Vec3::new(2.5, 3.5, 3.5), Vec3::X, 20.0)
        .unwrap();
    assert_eq!(hit.block, IVec3::new(10, 3, 3));
    assert_eq!(hit.normal, IVec3::NEG_X);
    assert_eq!(hit.adjacent(), IVec3::new(9, 3, 3));
    assert!((hit.distance - 7.5).abs() < 1e-4);

    // Diagonal, through a chunk border, from negative coordinates.
    let target = Vec3::new(10.5, 3.5, 3.5);
    let origin = Vec3::new(-6.2, 9.1, -4.7);
    let hit = world
        .raycast(&registry, origin, target - origin, 64.0)
        .unwrap();
    assert_eq!(hit.block, IVec3::new(10, 3, 3));

    assert!(world
        .raycast(&registry, Vec3::new(2.5, 3.5, 3.5), Vec3::X, 5.0)
        .is_none());
    assert!(world
        .raycast(&registry, Vec3::new(2.5, 3.5, 3.5), Vec3::Y, 20.0)
        .is_none());
}

#[test]
fn terrain_is_deterministic_and_layered() {
    let mut registry = BlockRegistry::default();
    let terrain = Terrain {
        seed: 42,
        grass: registry.register(Block::pillar("grass", 0, 1, 2)),
        dirt: registry.register(Block::uniform("dirt", 2)),
        stone: registry.register(Block::uniform("stone", 3)),
        sand: registry.register(Block::uniform("sand", 4)),
    };
    assert_eq!(terrain.generate(IVec3::ZERO), terrain.generate(IVec3::ZERO));
    assert!(terrain
        .generate(IVec3::new(0, 3, 0))
        .is_uniform(BlockId::AIR));

    let bottom = terrain.generate(IVec3::ZERO);
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            assert_eq!(
                bottom.get(IVec3::new(x, 0, z)),
                terrain.stone,
                "the floor is sealed"
            );
        }
    }
}
