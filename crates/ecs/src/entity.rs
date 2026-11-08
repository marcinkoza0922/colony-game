use serde::{Deserialize, Serialize};

/// A generational handle to an entity. Cheap to copy; becomes stale once the
/// entity is despawned (the slot's generation is bumped on reuse).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// A handle that never refers to a live entity.
    pub const DANGLING: Entity = Entity { index: u32::MAX, generation: u32::MAX };

    #[inline]
    pub fn index(self) -> u32 {
        self.index
    }

    #[inline]
    pub fn generation(self) -> u32 {
        self.generation
    }

    pub fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    pub fn from_bits(bits: u64) -> Self {
        Entity { index: bits as u32, generation: (bits >> 32) as u32 }
    }
}

impl std::fmt::Display for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Entity allocator. Freed slots are reused LIFO, which keeps allocation
/// deterministic for a given sequence of spawns/despawns.
#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct Entities {
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,
    live_count: u32,
}

impl Entities {
    pub fn alloc(&mut self) -> Entity {
        self.live_count += 1;
        if let Some(index) = self.free.pop() {
            let i = index as usize;
            self.alive[i] = true;
            Entity { index, generation: self.generations[i] }
        } else {
            let index = self.generations.len() as u32;
            self.generations.push(0);
            self.alive.push(true);
            Entity { index, generation: 0 }
        }
    }

    /// Frees the entity. Returns false if it was already dead/stale.
    pub fn free(&mut self, e: Entity) -> bool {
        if !self.is_alive(e) {
            return false;
        }
        let i = e.index as usize;
        self.alive[i] = false;
        self.generations[i] = self.generations[i].wrapping_add(1);
        self.free.push(e.index);
        self.live_count -= 1;
        true
    }

    #[inline]
    pub fn is_alive(&self, e: Entity) -> bool {
        let i = e.index as usize;
        i < self.alive.len() && self.alive[i] && self.generations[i] == e.generation
    }

    pub fn len(&self) -> usize {
        self.live_count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    /// Iterates live entities in index order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive.iter().enumerate().filter(|(_, a)| **a).map(|(i, _)| Entity {
            index: i as u32,
            generation: self.generations[i],
        })
    }
}
