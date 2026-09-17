use std::{
    any::{type_name, TypeId},
    collections::{HashMap, HashSet},
};

/// What a single query touches. `with`/`without` are used to prove two queries can never
/// match the same entity, which lets e.g. `Query<&mut Transform, With<Camera>>` and
/// `Query<&mut Transform, Without<Camera>>` live in the same system.
#[derive(Default, Clone)]
pub struct FilteredAccess {
    reads: HashSet<TypeId>,
    writes: HashSet<TypeId>,
    filter_reads: HashSet<TypeId>,
    with: HashSet<TypeId>,
    without: HashSet<TypeId>,
    names: HashMap<TypeId, &'static str>,
    conflicts: Vec<&'static str>,
}

impl FilteredAccess {
    pub fn read<T: 'static>(&mut self) {
        let id = self.name::<T>();
        if self.writes.contains(&id) {
            self.conflicts.push(type_name::<T>());
        }
        self.reads.insert(id);
    }

    pub fn write<T: 'static>(&mut self) {
        let id = self.name::<T>();
        if self.reads.contains(&id) || self.writes.contains(&id) {
            self.conflicts.push(type_name::<T>());
        }
        self.writes.insert(id);
    }

    /// Reads done by filters (e.g. `Changed<T>`). They happen before an item is produced, so they
    /// may overlap with writes in the same query.
    pub fn filter_read<T: 'static>(&mut self) {
        let id = self.name::<T>();
        self.filter_reads.insert(id);
    }

    pub fn with<T: 'static>(&mut self) {
        let id = self.name::<T>();
        self.with.insert(id);
    }

    pub fn without<T: 'static>(&mut self) {
        let id = self.name::<T>();
        self.without.insert(id);
    }

    /// Merges only the data access of `other`, not its archetype requirements. Used by `Option<D>`.
    pub fn extend_data(&mut self, other: FilteredAccess) {
        self.reads.extend(other.reads);
        self.writes.extend(other.writes);
        self.filter_reads.extend(other.filter_reads);
        self.names.extend(other.names);
        self.conflicts.extend(other.conflicts);
    }

    fn name<T: 'static>(&mut self) -> TypeId {
        let id = TypeId::of::<T>();
        self.names.insert(id, type_name::<T>());
        id
    }

    fn is_disjoint(&self, other: &FilteredAccess) -> bool {
        !self.with.is_disjoint(&other.without) || !self.without.is_disjoint(&other.with)
    }

    fn conflict_with(&self, other: &FilteredAccess) -> Option<TypeId> {
        let self_reads = || self.reads.iter().chain(&self.filter_reads);
        let other_reads = || other.reads.iter().chain(&other.filter_reads);
        self.writes
            .iter()
            .find(|id| other.writes.contains(*id) || other_reads().any(|r| r == *id))
            .or_else(|| {
                other
                    .writes
                    .iter()
                    .find(|id| self_reads().any(|r| r == *id))
            })
            .copied()
    }
}

/// Everything a system touches. Built once when the system is initialized; conflicting access
/// panics right then instead of causing aliasing bugs at runtime.
pub struct Access {
    system: String,
    queries: Vec<FilteredAccess>,
    resource_reads: HashSet<TypeId>,
    resource_writes: HashSet<TypeId>,
}

impl Access {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            queries: Vec::new(),
            resource_reads: HashSet::new(),
            resource_writes: HashSet::new(),
        }
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    pub fn add_query(&mut self, query: FilteredAccess, query_name: &str) {
        if let Some(name) = query.conflicts.first() {
            panic!(
                "system `{}`: `{query_name}` accesses `{name}` both mutably and immutably",
                self.system
            );
        }
        for existing in &self.queries {
            if existing.is_disjoint(&query) {
                continue;
            }
            if let Some(id) = existing.conflict_with(&query) {
                let name = query.names.get(&id).or(existing.names.get(&id)).unwrap();
                panic!(
                    "system `{}`: `{query_name}` conflicts with another query over `{name}`. \
                     Merge the queries, or make them disjoint with `With`/`Without` filters.",
                    self.system
                );
            }
        }
        self.queries.push(query);
    }

    pub fn read_resource<T: 'static>(&mut self) {
        if self.resource_writes.contains(&TypeId::of::<T>()) {
            self.resource_conflict::<T>();
        }
        self.resource_reads.insert(TypeId::of::<T>());
    }

    pub fn write_resource<T: 'static>(&mut self) {
        let id = TypeId::of::<T>();
        if self.resource_reads.contains(&id) || self.resource_writes.contains(&id) {
            self.resource_conflict::<T>();
        }
        self.resource_writes.insert(id);
    }

    fn resource_conflict<T: 'static>(&self) -> ! {
        panic!(
            "system `{}`: resource `{}` is accessed mutably alongside another access to it",
            self.system,
            type_name::<T>()
        );
    }
}
