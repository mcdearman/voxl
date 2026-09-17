use std::fmt;

/// A handle to an entity. The generation detects stale handles after an index is recycled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Allocates entity ids and tracks which are alive.
#[derive(Default)]
pub struct Entities {
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,
    len: usize,
}

impl Entities {
    pub fn alloc(&mut self) -> Entity {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let i = index as usize;
            self.alive[i] = true;
            Entity {
                index,
                generation: self.generations[i],
            }
        } else {
            let index = u32::try_from(self.generations.len()).expect("entity index overflow");
            self.generations.push(0);
            self.alive.push(true);
            Entity {
                index,
                generation: 0,
            }
        }
    }

    pub fn free(&mut self, entity: Entity) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let i = entity.index as usize;
        self.alive[i] = false;
        // Bump the generation so any handle still pointing at this slot goes stale.
        self.generations[i] = self.generations[i].wrapping_add(1);
        self.free.push(entity.index);
        self.len -= 1;
        true
    }

    pub fn contains(&self, entity: Entity) -> bool {
        let i = entity.index as usize;
        i < self.alive.len() && self.alive[i] && self.generations[i] == entity.generation
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .enumerate()
            .filter(|(_, alive)| **alive)
            .map(|(i, _)| Entity {
                index: i as u32,
                generation: self.generations[i],
            })
    }
}
