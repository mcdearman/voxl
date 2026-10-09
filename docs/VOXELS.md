# Voxel edits

Terrain is generated: a chunk's blocks come from the `ChunkGenerator`, and a chunk nobody is
near is dropped and generated again when a `ChunkViewer` comes back. That only works for
chunks that still match the generator. This page covers what happens to the ones that don't.

## Edited chunks

`VoxelWorld::set_block` marks a chunk as edited the first time it really changes a block in
it. Setting a block to what it already is doesn't count. Setting a block and then setting it
back does: the world doesn't compare against the generator, it only notes that the chunk was
touched.

An edited chunk that streams out is kept in memory, run-length encoded, instead of being
discarded. When a viewer returns it is loaded from that copy, without asking the generator.
An unedited chunk behaves as before.

```rust
world.is_edited(chunk);        // edited, whether it is loaded right now or not
world.edited_chunk_count();
```

## Saving and loading

```rust
let world = app.world.resource::<VoxelWorld>();
world.save_edits("saves/world.vxle")?;            // returns how many chunks were written

let world = app.world.resource_mut::<VoxelWorld>();
world.load_edits("saves/world.vxle")?;            // returns how many chunks were read
```

Only edited chunks are written, loaded or not. The file is written beside its destination and
renamed over it, so a crash while saving leaves the previous file intact.

Loading a chunk that is currently loaded replaces its blocks and remeshes it and its
neighbours. Loading a chunk that isn't loaded keeps the saved blocks until it streams in. It
is safe to load before any chunks exist, which is the usual case when starting a saved world.

Loading adds to the world's edits; it doesn't remove any. An edited chunk the file doesn't
mention stays edited. To get exactly the saved world, load into a world with no edits.

A file that is cut short, isn't an edits file, or doesn't add up gives an `EditsError`, and the
world is left exactly as it was: the whole file is checked before any of it is applied.

`edits_to_bytes` and `load_edits_from_bytes` do the same without a file, for putting the edits
inside a larger save.

## The file

Little-endian throughout.

| Bytes | |
|-------|---|
| `VXLE` | Magic number |
| `u32` | Format version, 1 |
| `u32` | Chunk size the file was written with, 32 |
| `u32` | Number of chunks |

Then, for each chunk, in order of position:

| Bytes | |
|-------|---|
| `i32` × 3 | Chunk position |
| `u32` | Number of runs |
| `u16`, `u16` per run | Length, block id |

Runs cover the chunk's blocks in storage order (x fastest, then z, then y) and add up to
exactly 32,768 blocks. A chunk of a single block is one run.

## What isn't here yet

- Blocks are saved as their numeric ids, not their names. A game has to register its blocks
  in the same order every run, or a saved file means something else.
- A file is tied to the generator as well: it holds the edited chunks whole, so changing the
  seed leaves them as islands of the old terrain.
- All edited chunks live in memory and in one file. Region files, and writing chunks out as
  they unload, come later (see the [roadmap](ROADMAP.md)).
- Chunks replaced with `insert_chunk` are treated as generated, not edited, and
  `remove_chunk` discards a chunk's edits with it. Streaming uses `unload_chunk`, which keeps
  them.
- Saving and loading happen on the calling thread.
