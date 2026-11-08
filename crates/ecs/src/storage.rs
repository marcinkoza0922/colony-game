use crate::entity::Entity;

const EMPTY: u32 = u32::MAX;

/// Sparse-set component storage: O(1) insert/remove/lookup, dense iteration.
///
/// Iteration order is the dense order, which depends only on the sequence of
/// operations performed — deterministic for a deterministic simulation.
pub struct SparseSet<T> {
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    data: Vec<T>,
}

impl<T> Default for SparseSet<T> {
    fn default() -> Self {
        Self { sparse: Vec::new(), entities: Vec::new(), data: Vec::new() }
    }
}

impl<T> SparseSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn dense_index(&self, e: Entity) -> Option<usize> {
        let slot = *self.sparse.get(e.index() as usize)?;
        if slot == EMPTY {
            return None;
        }
        let slot = slot as usize;
        (self.entities[slot] == e).then_some(slot)
    }

    /// Inserts or replaces the component for `e`, returning the old value.
    pub fn insert(&mut self, e: Entity, value: T) -> Option<T> {
        let i = e.index() as usize;
        if i >= self.sparse.len() {
            self.sparse.resize(i + 1, EMPTY);
        }
        let slot = self.sparse[i];
        if slot != EMPTY {
            let slot = slot as usize;
            if self.entities[slot] == e {
                return Some(std::mem::replace(&mut self.data[slot], value));
            }
            // Stale entry from an older generation: overwrite it in place.
            self.entities[slot] = e;
            self.data[slot] = value;
            return None;
        }
        self.sparse[i] = self.entities.len() as u32;
        self.entities.push(e);
        self.data.push(value);
        None
    }

    pub fn remove(&mut self, e: Entity) -> Option<T> {
        let slot = self.dense_index(e)?;
        let last = self.entities.len() - 1;
        if slot != last {
            let moved = self.entities[last];
            self.sparse[moved.index() as usize] = slot as u32;
        }
        self.sparse[e.index() as usize] = EMPTY;
        self.entities.swap_remove(slot);
        Some(self.data.swap_remove(slot))
    }

    #[inline]
    pub fn get(&self, e: Entity) -> Option<&T> {
        self.dense_index(e).map(|i| &self.data[i])
    }

    #[inline]
    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        self.dense_index(e).map(|i| &mut self.data[i])
    }

    #[inline]
    pub fn contains(&self, e: Entity) -> bool {
        self.dense_index(e).is_some()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn values(&self) -> &[T] {
        &self.data
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.entities.iter().copied().zip(self.data.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.entities.iter().copied().zip(self.data.iter_mut())
    }

    pub fn clear(&mut self) {
        self.sparse.clear();
        self.entities.clear();
        self.data.clear();
    }
}
