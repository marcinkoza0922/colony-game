//! Whole-world serialization. Components and resources opt in by registering
//! with a stable name; each is stored as an independent named blob so that
//! unknown or removed types are skipped on load instead of breaking the save.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::entity::{Entities, Entity};
use crate::world::{Component, World};

pub type SnapshotError = String;

fn config() -> bincode::config::Configuration {
    bincode::config::standard()
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, SnapshotError> {
    bincode::serde::encode_to_vec(value, config()).map_err(|e| e.to_string())
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, SnapshotError> {
    bincode::serde::decode_from_slice(bytes, config()).map(|(v, _)| v).map_err(|e| e.to_string())
}

type SaveFn = fn(&World) -> Result<Vec<u8>, SnapshotError>;
type LoadFn = fn(&mut World, &[u8]) -> Result<(), SnapshotError>;

struct Entry {
    name: &'static str,
    save: SaveFn,
    load: LoadFn,
}

/// Registry of serializable component and resource types.
#[derive(Default)]
pub struct SaveRegistry {
    components: Vec<Entry>,
    resources: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub entities: Entities,
    pub components: Vec<(String, Vec<u8>)>,
    pub resources: Vec<(String, Vec<u8>)>,
}

fn save_component<T: Component + Serialize>(w: &World) -> Result<Vec<u8>, SnapshotError> {
    let store = w.read::<T>();
    let items: Vec<(Entity, &T)> = store.iter().collect();
    encode(&items)
}

fn load_component<T: Component + DeserializeOwned>(w: &mut World, bytes: &[u8]) -> Result<(), SnapshotError> {
    let items: Vec<(Entity, T)> = decode(bytes)?;
    for (e, v) in items {
        if w.is_alive(e) {
            w.insert(e, v);
        }
    }
    Ok(())
}

fn save_resource<R: Serialize + 'static>(w: &World) -> Result<Vec<u8>, SnapshotError> {
    encode(&*w.res::<R>())
}

fn load_resource<R: DeserializeOwned + 'static>(w: &mut World, bytes: &[u8]) -> Result<(), SnapshotError> {
    let r: R = decode(bytes)?;
    w.insert_resource(r);
    Ok(())
}

impl SaveRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn component<T: Component + Serialize + DeserializeOwned>(&mut self, name: &'static str) -> &mut Self {
        assert!(!self.components.iter().any(|c| c.name == name), "duplicate component save name {name}");
        self.components.push(Entry { name, save: save_component::<T>, load: load_component::<T> });
        self
    }

    pub fn resource<R: Serialize + DeserializeOwned + 'static>(&mut self, name: &'static str) -> &mut Self {
        assert!(!self.resources.iter().any(|c| c.name == name), "duplicate resource save name {name}");
        self.resources.push(Entry { name, save: save_resource::<R>, load: load_resource::<R> });
        self
    }

    pub fn snapshot(&self, world: &World) -> Result<Snapshot, SnapshotError> {
        let mut components = Vec::with_capacity(self.components.len());
        for c in &self.components {
            components.push((c.name.to_string(), (c.save)(world).map_err(|e| format!("{}: {e}", c.name))?));
        }
        let mut resources = Vec::with_capacity(self.resources.len());
        for r in &self.resources {
            resources.push((r.name.to_string(), (r.save)(world).map_err(|e| format!("{}: {e}", r.name))?));
        }
        Ok(Snapshot { entities: world.entities().clone(), components, resources })
    }

    /// Replaces all entities with the snapshot's and overwrites registered
    /// resources present in it. Returns names of blobs that were skipped.
    pub fn restore(&self, world: &mut World, snap: &Snapshot) -> Result<Vec<String>, SnapshotError> {
        world.clear_entities();
        world.replace_entities(snap.entities.clone());
        let mut skipped = Vec::new();
        for (name, bytes) in &snap.components {
            match self.components.iter().find(|c| c.name == name) {
                Some(c) => (c.load)(world, bytes).map_err(|e| format!("{name}: {e}"))?,
                None => skipped.push(name.clone()),
            }
        }
        for (name, bytes) in &snap.resources {
            match self.resources.iter().find(|c| c.name == name) {
                Some(c) => (c.load)(world, bytes).map_err(|e| format!("{name}: {e}"))?,
                None => skipped.push(name.clone()),
            }
        }
        Ok(skipped)
    }

    pub fn save_bytes(&self, world: &World) -> Result<Vec<u8>, SnapshotError> {
        encode(&self.snapshot(world)?)
    }

    pub fn load_bytes(&self, world: &mut World, bytes: &[u8]) -> Result<Vec<String>, SnapshotError> {
        let snap: Snapshot = decode(bytes)?;
        self.restore(world, &snap)
    }

    /// FNV-1a hash of the serialized world, for determinism checks.
    pub fn state_hash(&self, world: &World) -> Result<u64, SnapshotError> {
        Ok(fnv1a(&self.save_bytes(world)?))
    }
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
