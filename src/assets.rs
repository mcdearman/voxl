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

    /// Rebuilds a handle from its id, for code (native plugins) that can only carry the number.
    pub(crate) fn from_id(id: u32) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
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
    /// Which store this is, among all there have been: ids mean something only in their own.
    store: u64,
    items: HashMap<u32, T>,
    next_id: u32,
    modified: HashSet<u32>,
    removed: Vec<u32>,
}

impl<T> Default for Assets<T> {
    fn default() -> Self {
        static STORES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            store: STORES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            items: HashMap::new(),
            next_id: 0,
            modified: HashSet::new(),
            removed: Vec::new(),
        }
    }
}

impl<T> Assets<T> {
    /// A number no other store has, to tell this one's handles from another's.
    pub fn store(&self) -> u64 {
        self.store
    }

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

    /// Hands out a handle for an asset that isn't there yet (one still loading, say). `get`
    /// returns `None` for it until `set` fills it in.
    pub fn reserve(&mut self) -> Handle<T> {
        let id = self.next_id;
        self.next_id += 1;
        Handle {
            id,
            _marker: PhantomData,
        }
    }

    /// Puts an asset under a handle, replacing what was there if anything was.
    pub fn set(&mut self, handle: Handle<T>, asset: T) {
        self.items.insert(handle.id, asset);
        self.modified.insert(handle.id);
    }

    /// The next id that will be handed out. Everything added between two readings of this
    /// has an id in between.
    pub(crate) fn next_id(&self) -> u32 {
        self.next_id
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

    pub(crate) fn contains_id(&self, id: u32) -> bool {
        self.items.contains_key(&id)
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

/// A handle to an asset the `AssetServer` knows by name is saved as that name, so it means
/// the same thing in a later run. Any other handle (a mesh built in code, say) is saved as
/// its id, which only holds while the same assets are made in the same order.
impl<T: 'static> crate::reflect::Reflect for Handle<T> {
    fn type_name() -> &'static str {
        "Handle"
    }

    fn to_value(&self) -> crate::reflect::Value {
        match crate::asset_server::name_in_scope(std::any::TypeId::of::<T>(), self.id) {
            Some((kind, name)) => crate::reflect::Value::Asset {
                kind: kind.to_owned(),
                name,
            },
            None => crate::reflect::Value::Int(self.id as i64),
        }
    }

    fn from_value(value: &crate::reflect::Value) -> Result<Self, crate::reflect::ReflectError> {
        if let crate::reflect::Value::Asset { name, .. } = value {
            // Scenes resolve these to ids before building components.
            return Err(crate::reflect::ReflectError::new(format!(
                "the asset `{name}` hasn't been loaded"
            )));
        }
        u32::from_value(value).map(Self::from_id)
    }

    fn schema() -> crate::reflect::Schema {
        crate::reflect::Schema::Int
    }
}
