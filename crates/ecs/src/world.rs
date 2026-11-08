use std::any::{Any, TypeId, type_name};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

use crate::entity::{Entities, Entity};
use crate::storage::SparseSet;

/// Anything `'static` can be a component.
pub trait Component: 'static {}
impl<T: 'static> Component for T {}

trait ErasedStorage: Any {
    fn remove_entity(&self, e: Entity);
    fn clear(&self);
    fn as_any(&self) -> &dyn Any;
}

struct Store<T>(RefCell<SparseSet<T>>);

impl<T: Component> ErasedStorage for Store<T> {
    fn remove_entity(&self, e: Entity) {
        self.0.borrow_mut().remove(e);
    }
    fn clear(&self) {
        self.0.borrow_mut().clear();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

type Command = Box<dyn FnOnce(&mut World)>;

/// The ECS world: entities, one sparse-set storage per component type, typed
/// singleton resources, and a deferred command queue.
///
/// Storages and resources sit behind `RefCell`s so that a system holding
/// `&World` can borrow several of them at once (e.g. write `Needs` while reading
/// `Position`). Structural changes during iteration go through [`World::defer`].
pub struct World {
    entities: RefCell<Entities>,
    storages: HashMap<TypeId, Box<dyn ErasedStorage>>,
    /// Registration order; used wherever deterministic iteration over storages
    /// matters (despawn, clear).
    storage_order: Vec<TypeId>,
    resources: HashMap<TypeId, RefCell<Box<dyn Any>>>,
    commands: RefCell<Vec<Command>>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: RefCell::new(Entities::default()),
            storages: HashMap::new(),
            storage_order: Vec::new(),
            resources: HashMap::new(),
            commands: RefCell::new(Vec::new()),
        }
    }

    // ---------------------------------------------------------------- entities

    /// Registers a component type. Idempotent.
    pub fn register<T: Component>(&mut self) {
        let id = TypeId::of::<T>();
        if let std::collections::hash_map::Entry::Vacant(e) = self.storages.entry(id) {
            e.insert(Box::new(Store::<T>(RefCell::new(SparseSet::new()))));
            self.storage_order.push(id);
        }
    }

    pub fn is_registered<T: Component>(&self) -> bool {
        self.storages.contains_key(&TypeId::of::<T>())
    }

    /// Allocates a new, empty entity. Usable through `&World`, so systems can
    /// reserve ids and attach components later via [`World::defer`].
    pub fn spawn_empty(&self) -> Entity {
        self.entities.borrow_mut().alloc()
    }

    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let e = self.spawn_empty();
        bundle.insert_into(self, e);
        e
    }

    /// Reserves an entity now and inserts the bundle when commands are applied.
    pub fn spawn_deferred<B: Bundle>(&self, bundle: B) -> Entity {
        let e = self.spawn_empty();
        self.defer(move |w| bundle.insert_into(w, e));
        e
    }

    pub fn despawn(&mut self, e: Entity) -> bool {
        if !self.entities.borrow_mut().free(e) {
            return false;
        }
        for id in &self.storage_order {
            self.storages[id].remove_entity(e);
        }
        true
    }

    #[inline]
    pub fn is_alive(&self, e: Entity) -> bool {
        self.entities.borrow().is_alive(e)
    }

    pub fn entity_count(&self) -> usize {
        self.entities.borrow().len()
    }

    pub fn entities(&self) -> Ref<'_, Entities> {
        self.entities.borrow()
    }

    pub(crate) fn replace_entities(&mut self, entities: Entities) {
        *self.entities.get_mut() = entities;
    }

    /// Removes every entity and component (resources are kept).
    pub fn clear_entities(&mut self) {
        for id in &self.storage_order {
            self.storages[id].clear();
        }
        *self.entities.get_mut() = Entities::default();
    }

    // -------------------------------------------------------------- components

    fn store<T: Component>(&self) -> &RefCell<SparseSet<T>> {
        let s = self
            .storages
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("component `{}` not registered", type_name::<T>()));
        &s.as_any().downcast_ref::<Store<T>>().unwrap().0
    }

    /// Shared borrow of a whole component storage.
    #[track_caller]
    pub fn read<T: Component>(&self) -> Ref<'_, SparseSet<T>> {
        self.store::<T>()
            .try_borrow()
            .unwrap_or_else(|_| panic!("`{}` already mutably borrowed", type_name::<T>()))
    }

    /// Exclusive borrow of a whole component storage.
    #[track_caller]
    pub fn write<T: Component>(&self) -> RefMut<'_, SparseSet<T>> {
        self.store::<T>()
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("`{}` already borrowed", type_name::<T>()))
    }

    pub fn insert<T: Component>(&mut self, e: Entity, value: T) -> Option<T> {
        debug_assert!(self.is_alive(e), "insert on dead entity {e}");
        self.register::<T>();
        self.store::<T>().borrow_mut().insert(e, value)
    }

    pub fn remove<T: Component>(&mut self, e: Entity) -> Option<T> {
        self.store::<T>().borrow_mut().remove(e)
    }

    pub fn has<T: Component>(&self, e: Entity) -> bool {
        self.read::<T>().contains(e)
    }

    pub fn get<T: Component>(&self, e: Entity) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.read::<T>(), |s| s.get(e)).ok()
    }

    pub fn get_mut<T: Component>(&self, e: Entity) -> Option<RefMut<'_, T>> {
        RefMut::filter_map(self.write::<T>(), |s| s.get_mut(e)).ok()
    }

    /// Clone a component out (convenience for small `Clone` components).
    pub fn get_cloned<T: Component + Clone>(&self, e: Entity) -> Option<T> {
        self.read::<T>().get(e).cloned()
    }

    // --------------------------------------------------------------- resources

    pub fn insert_resource<R: 'static>(&mut self, r: R) {
        self.resources.insert(TypeId::of::<R>(), RefCell::new(Box::new(r)));
    }

    pub fn remove_resource<R: 'static>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .map(|c| *c.into_inner().downcast::<R>().unwrap())
    }

    pub fn has_resource<R: 'static>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    #[track_caller]
    pub fn res<R: 'static>(&self) -> Ref<'_, R> {
        let cell = self
            .resources
            .get(&TypeId::of::<R>())
            .unwrap_or_else(|| panic!("resource `{}` missing", type_name::<R>()));
        let b = cell
            .try_borrow()
            .unwrap_or_else(|_| panic!("resource `{}` already mutably borrowed", type_name::<R>()));
        Ref::map(b, |b| b.downcast_ref::<R>().unwrap())
    }

    #[track_caller]
    pub fn res_mut<R: 'static>(&self) -> RefMut<'_, R> {
        let cell = self
            .resources
            .get(&TypeId::of::<R>())
            .unwrap_or_else(|| panic!("resource `{}` missing", type_name::<R>()));
        let b = cell
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("resource `{}` already borrowed", type_name::<R>()));
        RefMut::map(b, |b| b.downcast_mut::<R>().unwrap())
    }

    // ---------------------------------------------------------------- commands

    /// Queues a structural change to run at the next [`World::apply_commands`].
    pub fn defer(&self, f: impl FnOnce(&mut World) + 'static) {
        self.commands.borrow_mut().push(Box::new(f));
    }

    pub fn defer_insert<T: Component>(&self, e: Entity, value: T) {
        self.defer(move |w| {
            if w.is_alive(e) {
                w.insert(e, value);
            }
        });
    }

    pub fn defer_remove<T: Component>(&self, e: Entity) {
        self.defer(move |w| {
            w.remove::<T>(e);
        });
    }

    pub fn defer_despawn(&self, e: Entity) {
        self.defer(move |w| {
            w.despawn(e);
        });
    }

    /// Applies queued commands in FIFO order (commands may queue more commands;
    /// those run in the same call).
    pub fn apply_commands(&mut self) {
        loop {
            let cmds = std::mem::take(self.commands.get_mut());
            if cmds.is_empty() {
                break;
            }
            for c in cmds {
                c(self);
            }
        }
    }
}

/// A group of components inserted together (tuples of components).
pub trait Bundle: 'static {
    fn insert_into(self, world: &mut World, e: Entity);
}

macro_rules! impl_bundle {
    ($($t:ident),*) => {
        impl<$($t: Component),*> Bundle for ($($t,)*) {
            #[allow(non_snake_case)]
            fn insert_into(self, world: &mut World, e: Entity) {
                let ($($t,)*) = self;
                $( world.insert(e, $t); )*
            }
        }
    };
}

impl_bundle!(A);
impl_bundle!(A, B);
impl_bundle!(A, B, C);
impl_bundle!(A, B, C, D);
impl_bundle!(A, B, C, D, E);
impl_bundle!(A, B, C, D, E, F);
impl_bundle!(A, B, C, D, E, F, G);
impl_bundle!(A, B, C, D, E, F, G, H);
impl_bundle!(A, B, C, D, E, F, G, H, I);
impl_bundle!(A, B, C, D, E, F, G, H, I, J);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L);
