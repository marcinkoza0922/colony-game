//! Deterministic, headless colony simulation.

pub mod ai;
pub mod components;
pub mod death;
pub mod debug;
pub mod defs;
pub mod env;
pub mod farm;
pub mod geom;
pub mod hazards;
pub mod health;
pub mod items;
pub mod jobs;
pub mod map;
pub mod medical;
pub mod orders;
pub mod path;
pub mod pawn_sys;
pub mod pawngen;
pub mod resources;
pub mod scenario;
pub mod rng;
pub mod social;
pub mod stats;
pub mod worldgen;

use std::path::Path;
use std::rc::Rc;

use ecs::{SaveRegistry, Schedule, World};

pub use defs::Defs;
pub use geom::P3;

use components::*;
use jobs::PawnDebug;
use map::Map;
use path::Pathfinder;
use resources::*;
use rng::SimRng;
use stats::StatCache;

const SAVE_MAGIC: &[u8; 8] = b"COLONY05";

pub struct Sim {
    pub world: World,
    schedule: Schedule,
    registry: SaveRegistry,
}

fn register_components(world: &mut World) {
    world.register::<Position>();
    world.register::<Pawn>();
    world.register::<Needs>();
    world.register::<Mood>();
    world.register::<Skills>();
    world.register::<Quirks>();
    world.register::<WorkPriorities>();
    world.register::<Mover>();
    world.register::<Facing>();
    world.register::<Inventory>();
    world.register::<MentalState>();
    world.register::<CurrentJob>();
    world.register::<Item>();
    world.register::<Plant>();
    world.register::<Building>();
    world.register::<Blueprint>();
    world.register::<Workbench>();
    world.register::<Bed>();
    world.register::<StatCache>();
    world.register::<PawnDebug>();
    world.register::<Appearance>();
    world.register::<health::Health>();
    world.register::<social::Social>();
    world.register::<death::Corpse>();
    world.register::<death::Grave>();
    world.register::<medical::CarryingPawn>();
}

fn save_registry() -> SaveRegistry {
    let mut r = SaveRegistry::new();
    r.component::<Position>("position")
        .component::<Pawn>("pawn")
        .component::<Needs>("needs")
        .component::<Mood>("mood")
        .component::<Skills>("skills")
        .component::<Quirks>("quirks")
        .component::<WorkPriorities>("work_priorities")
        .component::<Mover>("mover")
        .component::<Facing>("facing")
        .component::<Inventory>("inventory")
        .component::<MentalState>("mental_state")
        .component::<CurrentJob>("current_job")
        .component::<Item>("item")
        .component::<Plant>("plant")
        .component::<Building>("building")
        .component::<Blueprint>("blueprint")
        .component::<Workbench>("workbench")
        .component::<Bed>("bed")
        .component::<StatCache>("stat_cache")
        .component::<Appearance>("appearance")
        .component::<health::Health>("health")
        .component::<social::Social>("social")
        .component::<death::Corpse>("corpse")
        .component::<death::Grave>("grave")
        .component::<medical::CarryingPawn>("carrying_pawn")
        .resource::<Map>("map")
        .resource::<Clock>("clock")
        .resource::<SimRng>("rng")
        .resource::<Zones>("zones")
        .resource::<Reservations>("reservations")
        .resource::<GameLog>("log")
        .resource::<SimMeta>("meta")
        .resource::<env::Env>("env")
        .resource::<env::WeatherState>("weather");
    r
}

fn schedule() -> Schedule {
    let mut s = Schedule::new();
    s.add("clock", pawn_sys::sys_clock)
        .add("weather", env::sys_weather)
        .add("env", env::sys_env)
        .add("positions", pawn_sys::sys_positions)
        .add("corpses", death::sys_corpses)
        .add("health", health::sys_health)
        .add("hazards", hazards::sys_hazards)
        .add("stats", pawn_sys::sys_stats)
        .add("needs", pawn_sys::sys_needs)
        .add("mood", pawn_sys::sys_mood)
        .add("breaks", pawn_sys::sys_breaks)
        .add("social", social::sys_social)
        .add("plants", farm::sys_plants)
        .add("jobs", pawn_sys::sys_jobs);
    s
}

impl Sim {
    /// Creates an empty sim (no map) — mostly for loading saves.
    fn empty(defs: Rc<Defs>) -> Sim {
        let mut world = World::new();
        register_components(&mut world);
        world.insert_resource(defs);
        world.insert_resource(Clock::default());
        world.insert_resource(SimRng::new(0));
        world.insert_resource(Zones::default());
        world.insert_resource(Reservations::default());
        world.insert_resource(GameLog::default());
        world.insert_resource(PawnPositions::default());
        world.insert_resource(death::CorpsePositions::default());
        world.insert_resource(Pathfinder::default());
        world.insert_resource(Map::new(1, 1, 1));
        world.insert_resource(SimMeta { seed: 0, start: P3::default() });
        world.insert_resource(env::Env::default());
        world.insert_resource(env::Rooms::default());
        world.insert_resource(env::WeatherState::default());
        Sim { world, schedule: schedule(), registry: save_registry() }
    }

    /// Generates a new colony.
    pub fn new(defs: Defs, seed: u64, params: &worldgen::WorldGenParams) -> Sim {
        let defs = Rc::new(defs);
        let mut sim = Sim::empty(defs.clone());
        let w = &mut sim.world;
        w.insert_resource(SimRng::new(seed));
        let generated = worldgen::generate(&defs, seed, params);
        w.insert_resource(generated.map);
        w.insert_resource(SimMeta { seed, start: generated.start });
        for (p, def, growth) in generated.plants {
            let e = w.spawn((Plant::new(def, growth), Position(p)));
            w.res_mut::<Map>().cell_mut(p).plant = Some(e);
        }
        for (p, def, n) in generated.items {
            items::spawn_items(w, def, n, p);
        }
        // Starting colonists: one of each of the first three races.
        let start = generated.start;
        let races: Vec<_> = defs.races.ids().take(3).collect();
        let mut colonists = Vec::new();
        for (i, r) in races.into_iter().enumerate() {
            colonists.push(pawngen::generate_pawn(w, r, start.offset(i as i32 - 1, 0, 0)));
        }
        // A relative of one of them comes along.
        if !colonists.is_empty() {
            let i = w.res_mut::<SimRng>().index(colonists.len());
            let of = colonists[i];
            let race = w.get::<Pawn>(of).unwrap().race;
            let kin = pawngen::generate_pawn(w, race, start.offset(2, 0, 0));
            social::make_family(w, of, kin);
        }
        // Starting supplies.
        let supplies = [
            ("wood", 80),
            ("berries", 40),
            ("simple_meal", 12),
            ("scrap", 30),
            ("stone", 20),
            ("ashroot_seeds", 24),
            ("rustwheat_seeds", 16),
            ("threadflax_seeds", 8),
            ("gloomcap_spores", 8),
            ("mendleaf_seeds", 6),
            ("herbal_medicine", 6),
        ];
        for (i, (name, n)) in supplies.iter().enumerate() {
            if let Some(d) = defs.items.find(name) {
                items::spawn_items(w, d, *n, start.offset(i as i32 - 2, 2, 0));
            }
        }
        jobs::log(w, LogKind::Info, "Your colonists have arrived at the ruins.");
        // Settle the environment so the first frames show sensible values.
        env::refresh_rooms(w);
        for _ in 0..3 {
            env::update_env(w);
        }
        sim
    }

    pub fn load_defs(dir: &Path) -> Result<Defs, String> {
        Defs::load_dir(dir, None)
    }

    pub fn defs(&self) -> Rc<Defs> {
        self.world.res::<Rc<Defs>>().clone()
    }

    pub fn tick(&mut self) {
        self.schedule.run(&mut self.world);
    }

    pub fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    pub fn clock(&self) -> Clock {
        self.world.res::<Clock>().clone()
    }

    pub fn save_bytes(&self) -> Result<Vec<u8>, String> {
        let names = self.defs().name_tables();
        let body = defs::with_names(names, || self.registry.save_bytes(&self.world))?;
        let mut out = SAVE_MAGIC.to_vec();
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub fn load_bytes(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        if bytes.len() < 8 || &bytes[..8] != SAVE_MAGIC {
            return Err("not a colony save (bad magic)".into());
        }
        let defs = self.defs();
        // Load into a fresh world so a failed load leaves this sim untouched.
        let mut fresh = Sim::empty(defs.clone());
        let skipped =
            defs::with_names(defs.name_tables(), || fresh.registry.load_bytes(&mut fresh.world, &bytes[8..]))?;
        fresh.fixup_after_defs_change();
        *self = fresh;
        Ok(skipped)
    }

    pub fn state_hash(&self) -> u64 {
        let names = self.defs().name_tables();
        defs::with_names(names, || self.registry.state_hash(&self.world)).expect("hashable")
    }

    /// Hot-reload support: swap in new defs (ids of existing names are stable).
    pub fn set_defs(&mut self, defs: Defs) {
        self.world.insert_resource(Rc::new(defs));
        self.fixup_after_defs_change();
    }

    /// Grows per-pawn vectors to match def counts, restores non-saved debug
    /// components.
    fn fixup_after_defs_change(&mut self) {
        let defs = self.defs();
        let w = &mut self.world;
        let pawns: Vec<_> = w.read::<Pawn>().entities().to_vec();
        for p in pawns {
            if let Some(mut n) = w.get_mut::<Needs>(p)
                && n.values.len() < defs.needs.len()
            {
                n.values.resize(defs.needs.len(), 1.0);
            }
            if let Some(mut s) = w.get_mut::<Skills>(p)
                && s.levels.len() < defs.skills.len()
            {
                s.levels.resize(defs.skills.len(), SkillLevel::default());
            }
            if let Some(mut wp) = w.get_mut::<WorkPriorities>(p)
                && wp.0.len() < defs.work_types.len()
            {
                wp.0.resize(defs.work_types.len(), 3);
            }
            if !w.has::<PawnDebug>(p) {
                w.insert(p, PawnDebug::default());
            }
        }
    }
}
