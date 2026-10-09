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

/// What a generator would make: stone below y = 4, air above.
fn layered(stone: BlockId) -> ChunkData {
    let mut data = ChunkData::default();
    for z in 0..CHUNK_SIZE {
        for y in 0..4 {
            for x in 0..CHUNK_SIZE {
                data.set(IVec3::new(x, y, z), stone);
            }
        }
    }
    data
}

#[test]
fn a_chunk_survives_being_turned_into_runs_and_back() {
    let (_, stone) = registry();
    let mut data = layered(stone);
    data.set(IVec3::new(7, 20, 9), stone);
    let runs = data.to_runs();
    // The slab, the air up to the lone block, the block, and the air after it.
    assert_eq!(runs.len(), 4);
    assert_eq!(ChunkData::from_runs(&runs), Some(data));

    let uniform = ChunkData::Uniform(stone);
    assert_eq!(uniform.to_runs().len(), 1);
    assert_eq!(ChunkData::from_runs(&uniform.to_runs()), Some(uniform));

    assert_eq!(ChunkData::from_runs(&[(5, stone)]), None, "too few blocks");
    assert_eq!(ChunkData::from_runs(&[]), None);
}

#[test]
fn only_a_block_that_really_changes_makes_its_chunk_edited() {
    let (_, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(3, 3, 3), BlockId::AIR);
    assert_eq!(world.edited_chunk_count(), 0);

    world.set_block(IVec3::new(3, 3, 3), stone);
    world.set_block(IVec3::new(4, 3, 3), stone);
    assert!(world.is_edited(IVec3::ZERO));
    assert!(!world.is_edited(IVec3::X));
    assert_eq!(world.edited_chunk_count(), 1);
}

#[test]
fn an_edited_chunk_that_is_unloaded_comes_back_with_its_edits() {
    let (_, stone) = registry();
    let mut world = VoxelWorld::default();
    world.insert_generated(IVec3::ZERO, layered(stone));
    world.insert_generated(IVec3::X, layered(stone));
    world.set_block(IVec3::new(1, 10, 1), stone);
    world.set_block(IVec3::new(1, 0, 1), BlockId::AIR);

    assert!(world.unload_chunk(IVec3::ZERO));
    assert!(world.unload_chunk(IVec3::X));
    assert!(!world.unload_chunk(IVec3::X), "it is already gone");
    assert_eq!(world.chunk_count(), 0);
    assert!(world.is_edited(IVec3::ZERO), "kept while it is away");
    assert!(!world.is_edited(IVec3::X));
    assert_eq!(world.get_block(IVec3::new(1, 10, 1)), None);

    // The untouched chunk has to be generated again; the edited one does not.
    assert!(!world.restore_edited(IVec3::X));
    assert!(world.restore_edited(IVec3::ZERO));
    assert!(world.dirty.contains(&IVec3::ZERO), "it needs a mesh again");
    assert_eq!(world.get_block(IVec3::new(1, 10, 1)), Some(stone));
    assert_eq!(world.get_block(IVec3::new(1, 0, 1)), Some(BlockId::AIR));
    assert_eq!(world.get_block(IVec3::new(2, 0, 1)), Some(stone));
    assert!(
        world.is_edited(IVec3::ZERO),
        "and is still saved from now on"
    );

    // If the generator's version turns up anyway, the edits win.
    world.unload_chunk(IVec3::ZERO);
    world.insert_generated(IVec3::ZERO, layered(stone));
    assert_eq!(world.get_block(IVec3::new(1, 10, 1)), Some(stone));
    assert_eq!(world.edited_chunk_count(), 1);
}

#[test]
fn removing_or_replacing_a_chunk_forgets_its_edits() {
    let (_, stone) = registry();
    let mut world = empty_world();
    world.set_block(IVec3::new(1, 1, 1), stone);
    world.set_block(IVec3::new(40, 1, 1), stone);
    assert_eq!(world.edited_chunk_count(), 2);

    assert!(world.remove_chunk(IVec3::ZERO).is_some());
    world.insert_chunk(IVec3::X, ChunkData::default());
    assert_eq!(world.edited_chunk_count(), 0);
    assert!(!world.restore_edited(IVec3::ZERO));
}

#[test]
fn an_edit_is_still_there_when_the_viewer_walks_away_and_returns() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use glam::Mat4;

    use crate::transform::GlobalTransform;

    let (_, stone) = registry();
    let generated = Arc::new(AtomicUsize::new(0));
    let counter = generated.clone();
    let mut app = App::new();
    app.insert_resource(VoxelSettings {
        view_radius: 1,
        min_chunk_y: 0,
        max_chunk_y: 0,
    })
    .insert_resource(ChunkGenerator::new(move |chunk| {
        if chunk == IVec3::ZERO {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        ChunkData::default()
    }))
    .insert_resource(TaskPool::new(2))
    .init_resource::<VoxelWorld>()
    .init_resource::<ChunkStreaming>()
    .add_systems(Stage::PostUpdate, streaming::stream_chunks);
    let viewer = app
        .world
        .spawn((GlobalTransform(Mat4::IDENTITY), ChunkViewer));

    // Generation happens on worker threads, so frames are run until it has caught up.
    fn run_until(app: &mut App, done: impl Fn(&VoxelWorld) -> bool) {
        for _ in 0..5000 {
            app.update();
            if done(app.world.resource::<VoxelWorld>()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("streaming never got there");
    }

    let block = IVec3::new(3, 4, 5);
    run_until(&mut app, |world| world.contains_chunk(IVec3::ZERO));
    assert!(app
        .world
        .resource_mut::<VoxelWorld>()
        .set_block(block, stone));

    let far = Vec3::new(100.0 * CHUNK_SIZE as f32, 0.0, 0.0);
    app.world.get_mut::<GlobalTransform>(viewer).unwrap().0 = Mat4::from_translation(far);
    run_until(&mut app, |world| !world.contains_chunk(IVec3::ZERO));
    assert_eq!(app.world.resource::<VoxelWorld>().get_block(block), None);
    assert!(app.world.resource::<VoxelWorld>().is_edited(IVec3::ZERO));

    app.world.get_mut::<GlobalTransform>(viewer).unwrap().0 = Mat4::IDENTITY;
    run_until(&mut app, |world| world.contains_chunk(IVec3::ZERO));
    let world = app.world.resource::<VoxelWorld>();
    assert_eq!(world.get_block(block), Some(stone));
    assert_eq!(world.get_block(block + IVec3::X), Some(BlockId::AIR));
    assert_eq!(
        generated.load(Ordering::SeqCst),
        1,
        "the edited chunk was not generated a second time"
    );
}

/// A world with edits in a loaded chunk and in one that has streamed out, plus an untouched
/// chunk, and the blocks that were set.
fn edited_world(stone: BlockId) -> (VoxelWorld, [IVec3; 3]) {
    let blocks = [
        IVec3::new(1, 10, 1),
        IVec3::new(31, 0, 31),
        IVec3::new(-5, 9, 70),
    ];
    let mut world = VoxelWorld::default();
    for chunk in [IVec3::ZERO, IVec3::new(-1, 0, 2), IVec3::new(4, 0, 4)] {
        world.insert_generated(chunk, layered(stone));
    }
    world.set_block(blocks[0], stone);
    world.set_block(blocks[1], BlockId::AIR);
    world.set_block(blocks[2], stone);
    world.unload_chunk(IVec3::new(-1, 0, 2));
    (world, blocks)
}

#[test]
fn saved_edits_load_into_a_fresh_world_with_the_same_blocks() {
    let (_, stone) = registry();
    let (saved, blocks) = edited_world(stone);
    let path = std::env::temp_dir().join(format!("mira-edits-{}.vxle", std::process::id()));
    assert_eq!(saved.save_edits(&path).unwrap(), 2);

    // One of the edited chunks is already loaded in the new world, the other is not.
    let mut world = VoxelWorld::default();
    world.insert_generated(IVec3::ZERO, layered(stone));
    world.insert_generated(IVec3::Y, layered(stone));
    world.dirty.clear();
    let loaded = world.load_edits(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(loaded.unwrap(), 2);

    assert_eq!(world.chunk(IVec3::ZERO), saved.chunk(IVec3::ZERO));
    assert_eq!(world.get_block(blocks[0]), Some(stone));
    assert_eq!(world.get_block(blocks[1]), Some(BlockId::AIR));
    assert!(
        world.dirty.contains(&IVec3::ZERO),
        "the replaced chunk is remeshed"
    );
    assert!(
        world.dirty.contains(&IVec3::Y),
        "and so is the chunk beside it"
    );

    // The other chunk waits until it streams in, and then has the saved blocks rather than
    // the generator's.
    assert_eq!(world.get_block(blocks[2]), None);
    assert!(world.is_edited(IVec3::new(-1, 0, 2)));
    world.insert_generated(IVec3::new(-1, 0, 2), layered(stone));
    assert_eq!(world.get_block(blocks[2]), Some(stone));
    assert_eq!(world.get_block(blocks[2] - IVec3::Y * 9), Some(stone));

    // Saving what was loaded gives the same file back.
    assert_eq!(world.edits_to_bytes(), saved.edits_to_bytes());
}

#[test]
fn a_chunk_nobody_edited_is_not_saved() {
    let (_, stone) = registry();
    let (saved, _) = edited_world(stone);
    let mut world = VoxelWorld::default();
    assert_eq!(
        world
            .load_edits_from_bytes(&saved.edits_to_bytes())
            .unwrap(),
        2
    );
    assert!(!world.is_edited(IVec3::new(4, 0, 4)));
    assert!(!world.restore_edited(IVec3::new(4, 0, 4)));

    // With no edits at all there is only the header.
    let mut untouched = VoxelWorld::default();
    untouched.insert_generated(IVec3::ZERO, layered(stone));
    assert_eq!(untouched.edits_to_bytes().len(), 16);
    assert_eq!(
        world
            .load_edits_from_bytes(&untouched.edits_to_bytes())
            .unwrap(),
        0
    );
    assert_eq!(
        world.edited_chunk_count(),
        2,
        "loading adds edits, it doesn't clear any"
    );
}

#[test]
fn truncated_or_garbage_edits_are_refused_and_change_nothing() {
    let (_, stone) = registry();
    let (saved, blocks) = edited_world(stone);
    let bytes = saved.edits_to_bytes();

    let mut world = VoxelWorld::default();
    world.insert_generated(IVec3::ZERO, layered(stone));
    world.dirty.clear();
    let mut refuse = |bytes: &[u8], why: &str| {
        let error = world.load_edits_from_bytes(bytes).expect_err(why);
        assert_eq!(
            world.edited_chunk_count(),
            0,
            "{why}: something was applied"
        );
        assert!(world.dirty.is_empty(), "{why}");
        assert_eq!(world.get_block(blocks[0]), Some(BlockId::AIR), "{why}");
        error
    };

    // Cut off anywhere, including in the middle of the second chunk after a whole first one.
    for length in 0..bytes.len() {
        refuse(&bytes[..length], "truncated");
    }
    assert!(matches!(refuse(b"", "empty"), EditsError::NotEdits));
    assert!(matches!(
        refuse(b"{ \"entities\": [] }", "some other file"),
        EditsError::NotEdits
    ));

    let mut newer = bytes.clone();
    newer[4] = 9;
    assert!(matches!(refuse(&newer, "version"), EditsError::Version(9)));
    let mut other_size = bytes.clone();
    other_size[8] = 16;
    assert!(matches!(
        refuse(&other_size, "chunk size"),
        EditsError::ChunkSize(16)
    ));

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        refuse(&trailing, "trailing"),
        EditsError::Corrupt(_)
    ));
    // The first run of the first chunk starts after the header, a position and a run count.
    let mut short_run = bytes.clone();
    short_run[32] ^= 1;
    assert!(matches!(refuse(&short_run, "runs"), EditsError::Corrupt(_)));
    // A chunk count far beyond what the data holds must not be believed.
    let mut many = bytes.clone();
    many[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(refuse(&many, "count"), EditsError::Corrupt(_)));
    // The same chunk twice: a header for two chunks, then one chunk's bytes repeated.
    let mut single = VoxelWorld::default();
    single.insert_generated(IVec3::ZERO, layered(stone));
    single.set_block(blocks[0], stone);
    let one = single.edits_to_bytes();
    let mut twice = one.clone();
    twice[12] = 2;
    twice.extend_from_slice(&one[16..]);
    assert!(matches!(
        refuse(&twice, "duplicate"),
        EditsError::Corrupt(_)
    ));

    assert!(matches!(
        world.load_edits(std::env::temp_dir().join("mira-no-such-edits.vxle")),
        Err(EditsError::Io(_))
    ));
    // And the untouched data still loads.
    assert_eq!(world.load_edits_from_bytes(&bytes).unwrap(), 2);
}
