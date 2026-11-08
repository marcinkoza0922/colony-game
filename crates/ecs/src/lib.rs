//! A small, purpose-built ECS for a deterministic, single-threaded colony sim.
//!
//! - [`Entity`]: generational handle.
//! - [`SparseSet`]: per-component storage (O(1) add/remove, dense iteration).
//! - [`World`]: storages + resources behind `RefCell`s, deferred commands.
//! - [`Schedule`]: ordered list of systems, commands applied between them.
//! - [`SaveRegistry`]: opt-in, name-keyed serialization of the world.

mod entity;
mod schedule;
mod snapshot;
mod storage;
mod world;

pub use entity::{Entities, Entity};
pub use schedule::{Schedule, System};
pub use snapshot::{SaveRegistry, Snapshot, SnapshotError, decode, encode, fnv1a};
pub use storage::SparseSet;
pub use world::{Bundle, Component, World};

#[cfg(test)]
mod tests;
