use crate::world::World;

pub type System = fn(&mut World);

/// An ordered list of systems. Deferred commands are applied after each system
/// so later systems observe earlier structural changes.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(&'static str, System)>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &'static str, system: System) -> &mut Self {
        self.systems.push((name, system));
        self
    }

    pub fn run(&self, world: &mut World) {
        for (_, system) in &self.systems {
            system(world);
            world.apply_commands();
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.systems.iter().map(|(n, _)| *n)
    }
}
