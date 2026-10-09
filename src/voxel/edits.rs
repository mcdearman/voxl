//! Saving and loading the chunks a player has edited.
//!
//! Only edited chunks are written: everything else comes back from the generator. The file is
//! little-endian throughout:
//!
//! ```text
//! "VXLE"                      magic
//! u32                         format version (1)
//! u32                         chunk size the file was written with
//! u32                         number of chunks
//! per chunk:
//!     i32 × 3                 chunk position
//!     u32                     number of runs
//!     per run: u16, u16       length, block id
//! ```
//!
//! A chunk's runs cover its blocks in storage order (x fastest, then z, then y) and must add
//! up to exactly one chunk.

use std::{collections::HashSet, fmt, fs, io, path::Path};

use glam::IVec3;

use super::{
    block::BlockId,
    chunk::{runs_fill_a_chunk, Runs, CHUNK_SIZE, VOLUME},
    world::VoxelWorld,
};

const MAGIC: &[u8; 4] = b"VXLE";
const VERSION: u32 = 1;

/// Why saved edits couldn't be loaded.
#[derive(Debug)]
pub enum EditsError {
    Io(io::Error),
    /// The data doesn't start the way saved edits do, so it is some other kind of file.
    NotEdits,
    /// Written by a version of the format this build doesn't read.
    Version(u32),
    /// Written by a build whose chunks are a different size.
    ChunkSize(u32),
    /// Cut short, or the contents don't add up.
    Corrupt(&'static str),
}

impl fmt::Display for EditsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::NotEdits => write!(f, "not a voxel edits file"),
            Self::Version(version) => write!(
                f,
                "voxel edits format version {version}, but this build reads version {VERSION}"
            ),
            Self::ChunkSize(size) => write!(
                f,
                "voxel edits for chunks of size {size}, but chunks here are {CHUNK_SIZE}"
            ),
            Self::Corrupt(what) => write!(f, "corrupt voxel edits: {what}"),
        }
    }
}

impl std::error::Error for EditsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for EditsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads fixed-size pieces off the front of the data, refusing to run past the end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], EditsError> {
        if self.0.len() < count {
            return Err(EditsError::Corrupt("the data ends early"));
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], EditsError> {
        Ok(self
            .take(N)?
            .try_into()
            .expect("take returns what was asked for"))
    }

    fn u16(&mut self) -> Result<u16, EditsError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, EditsError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, EditsError> {
        Ok(i32::from_le_bytes(self.array()?))
    }
}

/// Reads and checks everything before anything is applied, so bad data changes nothing.
fn parse(bytes: &[u8]) -> Result<Vec<(IVec3, Runs)>, EditsError> {
    let mut reader = Reader(bytes);
    if reader.take(MAGIC.len()).ok() != Some(MAGIC.as_slice()) {
        return Err(EditsError::NotEdits);
    }
    let version = reader.u32()?;
    if version != VERSION {
        return Err(EditsError::Version(version));
    }
    let chunk_size = reader.u32()?;
    if chunk_size != CHUNK_SIZE as u32 {
        return Err(EditsError::ChunkSize(chunk_size));
    }

    // Sizes read from the file are never trusted with an allocation: the vectors grow only
    // as data that is really there is read.
    let chunk_count = reader.u32()?;
    let mut chunks = Vec::new();
    let mut seen = HashSet::new();
    for _ in 0..chunk_count {
        let chunk = IVec3::new(reader.i32()?, reader.i32()?, reader.i32()?);
        if !seen.insert(chunk) {
            return Err(EditsError::Corrupt("a chunk appears twice"));
        }
        let run_count = reader.u32()? as usize;
        if run_count > VOLUME {
            return Err(EditsError::Corrupt("a chunk has more runs than blocks"));
        }
        let mut runs = Runs::new();
        for _ in 0..run_count {
            let length = reader.u16()?;
            let id = BlockId(reader.u16()?);
            if id == BlockId::UNLOADED {
                return Err(EditsError::Corrupt("a block id that is never stored"));
            }
            runs.push((length, id));
        }
        if !runs_fill_a_chunk(&runs) {
            return Err(EditsError::Corrupt(
                "a chunk's runs don't add up to a chunk",
            ));
        }
        chunks.push((chunk, runs));
    }
    if !reader.0.is_empty() {
        return Err(EditsError::Corrupt("there is data after the last chunk"));
    }
    Ok(chunks)
}

impl VoxelWorld {
    /// Every edited chunk, loaded or not, in the format described in this module.
    pub fn edits_to_bytes(&self) -> Vec<u8> {
        let chunks = self.edited_chunks();
        let mut bytes = Vec::new();
        bytes.extend(MAGIC);
        bytes.extend(VERSION.to_le_bytes());
        bytes.extend((CHUNK_SIZE as u32).to_le_bytes());
        bytes.extend((chunks.len() as u32).to_le_bytes());
        for (chunk, runs) in chunks {
            for coordinate in chunk.to_array() {
                bytes.extend(coordinate.to_le_bytes());
            }
            bytes.extend((runs.len() as u32).to_le_bytes());
            for (length, id) in runs {
                bytes.extend(length.to_le_bytes());
                bytes.extend(id.0.to_le_bytes());
            }
        }
        bytes
    }

    /// Applies saved edits, and returns how many chunks they held. A chunk that is loaded is
    /// replaced and remeshed; one that isn't gets its saved blocks when it streams in.
    ///
    /// Edits already in the world to chunks the data doesn't mention are left alone. If the
    /// data is bad, nothing is changed.
    pub fn load_edits_from_bytes(&mut self, bytes: &[u8]) -> Result<usize, EditsError> {
        let chunks = parse(bytes)?;
        let count = chunks.len();
        for (chunk, runs) in chunks {
            self.set_edited_chunk(chunk, runs);
        }
        Ok(count)
    }

    /// Writes every edited chunk to a file, and returns how many there were.
    pub fn save_edits(&self, path: impl AsRef<Path>) -> io::Result<usize> {
        let path = path.as_ref();
        // Written beside the file and then renamed over it, so a crash part-way through
        // leaves the previous save intact rather than a truncated one.
        let mut partial = path.as_os_str().to_owned();
        partial.push(".part");
        fs::write(&partial, self.edits_to_bytes())?;
        fs::rename(&partial, path)?;
        Ok(self.edited_chunk_count())
    }

    /// Reads a file written by [`save_edits`](Self::save_edits). See
    /// [`load_edits_from_bytes`](Self::load_edits_from_bytes) for what loading does.
    pub fn load_edits(&mut self, path: impl AsRef<Path>) -> Result<usize, EditsError> {
        self.load_edits_from_bytes(&fs::read(path)?)
    }
}
