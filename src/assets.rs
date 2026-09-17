use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

/// A typed id for an asset stored in `Assets<T>`.
pub struct Handle<T> {
    id: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub fn id(&self) -> u32 {
        self.id
    }
}

// Manual impls so `T` doesn't need to implement these traits.
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle<{}>({})", std::any::type_name::<T>(), self.id)
    }
}

/// Storage for one asset type. Tracks modifications so GPU-side copies can be kept in sync.
pub struct Assets<T> {
    items: HashMap<u32, T>,
    next_id: u32,
    modified: HashSet<u32>,
    removed: Vec<u32>,
}

impl<T> Default for Assets<T> {
    fn default() -> Self {
        Self {
            items: HashMap::new(),
            next_id: 0,
            modified: HashSet::new(),
            removed: Vec::new(),
        }
    }
}

impl<T> Assets<T> {
    pub fn add(&mut self, asset: T) -> Handle<T> {
        let id = self.next_id;
        self.next_id += 1;
        self.items.insert(id, asset);
        self.modified.insert(id);
        Handle {
            id,
            _marker: PhantomData,
        }
    }

    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        self.items.get(&handle.id)
    }

    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        let item = self.items.get_mut(&handle.id)?;
        self.modified.insert(handle.id);
        Some(item)
    }

    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        let item = self.items.remove(&handle.id)?;
        self.modified.remove(&handle.id);
        self.removed.push(handle.id);
        Some(item)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn get_by_id(&self, id: u32) -> Option<&T> {
        self.items.get(&id)
    }

    /// Returns the ids added or modified, and the ids removed, since the last call.
    pub(crate) fn take_changes(&mut self) -> (Vec<u32>, Vec<u32>) {
        (
            self.modified.drain().collect(),
            std::mem::take(&mut self.removed),
        )
    }
}
