//! Data-driven content definitions loaded from `assets/defs/<kind>/*.ron`.
//!
//! Defs are referenced at runtime by small typed indices ([`DefId`]). During
//! loading *and* save/load, a thread-local name table is installed so that a
//! `DefId` (de)serializes as the def's **name**. That gives us:
//! - cross-references between def files written as plain names, resolved and
//!   validated at load time, and
//! - saves that survive reordering / editing of def files.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Index;
use std::path::Path;
use std::rc::Rc;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

// ------------------------------------------------------------------- DefId

pub trait Def: Clone + 'static {
    const KIND: &'static str;
    fn name(&self) -> &str;
}

pub struct DefId<T> {
    idx: u16,
    _p: PhantomData<fn() -> T>,
}

impl<T> DefId<T> {
    pub const fn from_index(idx: usize) -> Self {
        Self { idx: idx as u16, _p: PhantomData }
    }
    #[inline]
    pub fn index(self) -> usize {
        self.idx as usize
    }
}

impl<T> Clone for DefId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for DefId<T> {}
impl<T> PartialEq for DefId<T> {
    fn eq(&self, o: &Self) -> bool {
        self.idx == o.idx
    }
}
impl<T> Eq for DefId<T> {}
impl<T> PartialOrd for DefId<T> {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl<T> Ord for DefId<T> {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.idx.cmp(&o.idx)
    }
}
impl<T> std::hash::Hash for DefId<T> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.idx.hash(h)
    }
}
impl<T: Def> fmt::Debug for DefId<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match name_of(T::KIND, self.idx) {
            Some(n) => write!(f, "{}:{}", T::KIND, n),
            None => write!(f, "{}#{}", T::KIND, self.idx),
        }
    }
}

#[derive(Default, Debug)]
pub struct KindNames {
    pub names: Vec<String>,
    pub index: HashMap<String, u16>,
}

/// Name tables for every def kind, installed thread-locally while
/// (de)serializing anything that contains `DefId`s.
#[derive(Default, Debug)]
pub struct NameTables {
    kinds: HashMap<&'static str, KindNames>,
}

impl NameTables {
    fn add(&mut self, kind: &'static str, names: Vec<String>) {
        let index = names.iter().enumerate().map(|(i, n)| (n.clone(), i as u16)).collect();
        self.kinds.insert(kind, KindNames { names, index });
    }
}

thread_local! {
    static NAMES: RefCell<Option<Rc<NameTables>>> = const { RefCell::new(None) };
}

/// Runs `f` with `tables` installed for DefId (de)serialization.
pub fn with_names<R>(tables: Rc<NameTables>, f: impl FnOnce() -> R) -> R {
    let prev = NAMES.with(|n| n.borrow_mut().replace(tables));
    let r = f();
    NAMES.with(|n| *n.borrow_mut() = prev);
    r
}

fn name_of(kind: &'static str, idx: u16) -> Option<String> {
    NAMES.with(|n| {
        n.borrow().as_ref().and_then(|t| t.kinds.get(kind)).and_then(|k| k.names.get(idx as usize).cloned())
    })
}

fn id_of(kind: &'static str, name: &str) -> Result<u16, String> {
    NAMES.with(|n| {
        let b = n.borrow();
        let t = b.as_ref().ok_or_else(|| format!("no def name table installed while reading {kind} `{name}`"))?;
        let k = t.kinds.get(kind).ok_or_else(|| format!("unknown def kind {kind}"))?;
        k.index.get(name).copied().ok_or_else(|| format!("unknown {kind} `{name}`"))
    })
}

impl<T: Def> Serialize for DefId<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match name_of(T::KIND, self.idx) {
            Some(n) => s.serialize_str(&n),
            None => Err(serde::ser::Error::custom(format!("no name for {}#{}", T::KIND, self.idx))),
        }
    }
}

impl<'de, T: Def> Deserialize<'de> for DefId<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<T: Def> Visitor<'_> for V<T> {
            type Value = DefId<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a {} name", T::KIND)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                id_of(T::KIND, v).map(|i| DefId::from_index(i as usize)).map_err(E::custom)
            }
        }
        d.deserialize_str(V::<T>(PhantomData))
    }
}

// ------------------------------------------------------------------ DefList

#[derive(Clone, Debug)]
pub struct DefList<T> {
    pub items: Vec<T>,
}

impl<T: Def> DefList<T> {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn find(&self, name: &str) -> Option<DefId<T>> {
        self.items.iter().position(|d| d.name() == name).map(DefId::from_index)
    }
    /// Panics with a helpful message if the def is missing (for code-required defs).
    #[track_caller]
    pub fn expect(&self, name: &str) -> DefId<T> {
        self.find(name).unwrap_or_else(|| panic!("required {} `{name}` missing from defs", T::KIND))
    }
    pub fn ids(&self) -> impl Iterator<Item = DefId<T>> + '_ {
        (0..self.items.len()).map(DefId::from_index)
    }
    pub fn iter(&self) -> impl Iterator<Item = (DefId<T>, &T)> {
        self.items.iter().enumerate().map(|(i, d)| (DefId::from_index(i), d))
    }
    pub fn names(&self) -> Vec<String> {
        self.items.iter().map(|d| d.name().to_string()).collect()
    }
}

impl<T> Index<DefId<T>> for DefList<T> {
    type Output = T;
    fn index(&self, id: DefId<T>) -> &T {
        &self.items[id.index()]
    }
}

// ------------------------------------------------------------- Def structs

pub type TerrainId = DefId<TerrainDef>;
pub type ItemId = DefId<ItemDef>;
pub type PlantId = DefId<PlantDef>;
pub type BuildingId = DefId<BuildingDef>;
pub type RecipeId = DefId<RecipeDef>;
pub type SkillId = DefId<SkillDef>;
pub type WorkTypeId = DefId<WorkTypeDef>;
pub type StatId = DefId<StatDef>;
pub type NeedId = DefId<NeedDef>;
pub type RaceId = DefId<RaceDef>;
pub type QuirkId = DefId<QuirkDef>;
pub type ThoughtId = DefId<ThoughtDef>;

fn one() -> f32 {
    1.0
}
fn one_u32() -> u32 {
    1
}
fn default_stack() -> u32 {
    75
}

#[derive(Deserialize, Clone, Debug)]
pub struct ItemAmount {
    pub item: ItemId,
    pub count: u32,
}

/// Situational predicate evaluated against a pawn (see `stats::eval_condition`).
#[derive(Deserialize, Clone, Debug, Default)]
pub enum Condition {
    #[default]
    Always,
    Underground,
    Outdoors,
    Night,
    Day,
    Alone,
    NearOthers(u32),
    NearPlants(u32),
    NeedBelow(NeedId, f32),
    NeedAbove(NeedId, f32),
    MoodBelow(f32),
    MoodAbove(f32),
    Working,
    Sleeping,
    HasQuirk(QuirkId),
    IsRace(RaceId),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub enum ModOp {
    Add(f32),
    Mul(f32),
}

#[derive(Deserialize, Clone, Debug)]
pub struct StatMod {
    pub stat: StatId,
    pub op: ModOp,
    #[serde(default)]
    pub when: Condition,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TerrainDef {
    pub name: String,
    pub label: String,
    pub sprite: String,
    /// Mining work multiplier.
    #[serde(default = "one")]
    pub hardness: f32,
    #[serde(default)]
    pub drops: Vec<ItemAmount>,
    #[serde(default = "one")]
    pub drop_chance: f32,
    #[serde(default)]
    pub fertile: bool,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemCategory {
    #[default]
    Resource,
    RawFood,
    Meal,
    Manufactured,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ItemDef {
    pub name: String,
    pub label: String,
    pub sprite: String,
    #[serde(default = "default_stack")]
    pub stack: u32,
    #[serde(default)]
    pub category: ItemCategory,
    /// Hunger restored per unit eaten (0 = inedible).
    #[serde(default)]
    pub nutrition: f32,
    /// Memory thought given when eaten.
    #[serde(default)]
    pub eat_thought: Option<ThoughtId>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct HarvestDef {
    pub yields: Vec<ItemAmount>,
    pub work: f32,
    /// Growth the plant resets to after harvesting.
    #[serde(default)]
    pub regrow_to: f32,
    /// Minimum growth for harvest to be possible.
    #[serde(default = "one")]
    pub min_growth: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PlantDef {
    pub name: String,
    pub label: String,
    pub sprite: String,
    #[serde(default)]
    pub tree: bool,
    pub grow_days: f32,
    #[serde(default)]
    pub chop_work: f32,
    #[serde(default)]
    pub chop_yield: Vec<ItemAmount>,
    #[serde(default)]
    pub harvest: Option<HarvestDef>,
    /// Worldgen density on fertile soil (0..1).
    #[serde(default)]
    pub wild_density: f32,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingKind {
    Wall,
    Door,
    Bed,
    Workbench,
    Stairs,
    Furniture,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BuildingDef {
    pub name: String,
    pub label: String,
    pub sprite: String,
    pub kind: BuildingKind,
    #[serde(default)]
    pub cost: Vec<ItemAmount>,
    #[serde(default)]
    pub work: f32,
    /// Shown in the build menu.
    #[serde(default = "default_true")]
    pub buildable: bool,
    /// Rest gain multiplier when sleeping here (beds).
    #[serde(default = "one")]
    pub rest_factor: f32,
    #[serde(default)]
    pub sleep_thought: Option<ThoughtId>,
    #[serde(default)]
    pub light: Option<LightDef>,
}

fn default_true() -> bool {
    true
}

impl BuildingDef {
    pub fn passable(&self) -> bool {
        !matches!(self.kind, BuildingKind::Wall)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct RecipeDef {
    pub name: String,
    pub label: String,
    pub bench: BuildingId,
    pub inputs: Vec<ItemAmount>,
    pub outputs: Vec<ItemAmount>,
    pub work: f32,
    pub skill: SkillId,
    pub work_type: WorkTypeId,
    pub speed_stat: StatId,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SkillDef {
    pub name: String,
    pub label: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct WorkTypeDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub skill: Option<SkillId>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct StatDef {
    pub name: String,
    pub label: String,
    pub default: f32,
    /// If set, value is multiplied by `skill_base + skill_per_level * level`.
    #[serde(default)]
    pub skill: Option<SkillId>,
    #[serde(default)]
    pub skill_base: f32,
    #[serde(default)]
    pub skill_per_level: f32,
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: Option<f32>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedRole {
    Food,
    Rest,
    Recreation,
}

#[derive(Deserialize, Clone, Debug)]
pub struct NeedDef {
    pub name: String,
    pub label: String,
    /// What job satisfies this need (if any).
    #[serde(default)]
    pub role: Option<NeedRole>,
    /// Passive change per day while condition holds (positive = fills).
    #[serde(default)]
    pub passive: Vec<(Condition, f32)>,
    /// Stat that multiplies the decay rate.
    #[serde(default)]
    pub rate_stat: Option<StatId>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RaceNeed {
    pub need: NeedId,
    /// Decay per day at rate 1.
    pub decay: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RaceDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Body model / sprite set.
    pub sprite: String,
    pub needs: Vec<RaceNeed>,
    #[serde(default)]
    pub base_stats: Vec<(StatId, f32)>,
    #[serde(default)]
    pub stat_mods: Vec<StatMod>,
    /// Learning-rate multipliers.
    #[serde(default)]
    pub aptitudes: Vec<(SkillId, f32)>,
    /// Starting skill bonus levels.
    #[serde(default)]
    pub skill_bonus: Vec<(SkillId, i32)>,
    #[serde(default)]
    pub innate_quirks: Vec<QuirkId>,
    /// Multiplies a quirk's base weight (0 = never rolls for this race).
    #[serde(default)]
    pub quirk_weights: Vec<(QuirkId, f32)>,
    pub adult_age: (u32, u32),
    #[serde(default)]
    pub appearance: AppearanceDef,
    pub first_names: Vec<String>,
    #[serde(default)]
    pub last_names: Vec<String>,
}

/// Colour palettes (hex strings) a race's colonists are rolled from.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppearanceDef {
    pub skin: Vec<String>,
    pub hair: Vec<String>,
    pub top: Vec<String>,
    pub bottom: Vec<String>,
    /// Number of head/hair styles baked for this race.
    pub styles: u8,
}

impl Default for AppearanceDef {
    fn default() -> Self {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect();
        Self {
            skin: v(&["#d9a066"]),
            hair: v(&["#4a3426"]),
            top: v(&["#6b7a8f", "#7a5a3a", "#5a6a4a", "#8a3a3a", "#6a5a7a"]),
            bottom: v(&["#4e4a42", "#3b3a40", "#5a4a3a"]),
            styles: 1,
        }
    }
}

/// Parses "#rrggbb" (falls back to magenta).
pub fn parse_hex(s: &str) -> [u8; 3] {
    let h = s.trim_start_matches('#');
    let p = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    match (p(0), p(2), p(4)) {
        (Some(r), Some(g), Some(b)) => [r, g, b],
        _ => [255, 0, 255],
    }
}

/// A point light emitted by a building.
#[derive(Deserialize, Clone, Debug)]
pub struct LightDef {
    /// Reach in cells.
    pub radius: f32,
    /// Linear-ish RGB intensity (can exceed 1).
    pub color: (f32, f32, f32),
    /// Height above the cell floor, in levels.
    #[serde(default = "half")]
    pub height: f32,
    /// Flicker amount 0..1 (fires).
    #[serde(default)]
    pub flicker: f32,
}

fn half() -> f32 {
    0.5
}

#[derive(Deserialize, Clone, Debug)]
pub struct QuirkDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "one")]
    pub weight: f32,
    /// Only granted via race `innate_quirks`, never rolled.
    #[serde(default)]
    pub innate: bool,
    #[serde(default)]
    pub conflicts: Vec<QuirkId>,
    #[serde(default)]
    pub stat_mods: Vec<StatMod>,
    #[serde(default)]
    pub aptitudes: Vec<(SkillId, f32)>,
    /// Multipliers on need decay.
    #[serde(default)]
    pub need_rates: Vec<(NeedId, f32)>,
    /// Shifts the night (sleep) window, in hours.
    #[serde(default)]
    pub sleep_shift: i32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ThoughtDef {
    pub name: String,
    pub label: String,
    pub mood: f32,
    /// Situational thought: active while this holds.
    #[serde(default)]
    pub when: Option<Condition>,
    /// Memory thought duration (hours). Ignored for situational thoughts.
    #[serde(default)]
    pub duration_hours: f32,
    #[serde(default = "one_u32")]
    pub stack_limit: u32,
    #[serde(default)]
    pub requires_quirk: Option<QuirkId>,
    #[serde(default)]
    pub requires_race: Option<RaceId>,
    #[serde(default)]
    pub blocked_by_quirks: Vec<QuirkId>,
    #[serde(default)]
    pub stat_mods: Vec<StatMod>,
}

macro_rules! impl_def {
    ($($t:ty => $kind:literal),* $(,)?) => {
        $(impl Def for $t {
            const KIND: &'static str = $kind;
            fn name(&self) -> &str { &self.name }
        })*
    };
}

impl_def! {
    TerrainDef => "terrain",
    ItemDef => "items",
    PlantDef => "plants",
    BuildingDef => "buildings",
    RecipeDef => "recipes",
    SkillDef => "skills",
    WorkTypeDef => "work_types",
    StatDef => "stats",
    NeedDef => "needs",
    RaceDef => "races",
    QuirkDef => "quirks",
    ThoughtDef => "thoughts",
}

// ------------------------------------------------------------------- Defs

/// Def ids that code refers to directly, resolved once at load.
#[derive(Clone, Debug)]
pub struct CoreIds {
    pub stat_move_speed: StatId,
    pub stat_work_speed: StatId,
    pub stat_mining_speed: StatId,
    pub stat_construction_speed: StatId,
    pub stat_plant_speed: StatId,
    pub stat_learn_rate: StatId,
    pub stat_break_threshold: StatId,
    pub skill_mining: SkillId,
    pub skill_construction: SkillId,
    pub skill_botany: SkillId,
    pub wt_hauling: WorkTypeId,
    pub wt_mining: WorkTypeId,
    pub wt_construction: WorkTypeId,
    pub wt_plants: WorkTypeId,
    pub stairs: BuildingId,
    pub thought_slept_ground: ThoughtId,
    pub thought_ate_without_table: Option<ThoughtId>,
}

#[derive(Clone, Debug)]
pub struct Defs {
    pub terrain: DefList<TerrainDef>,
    pub items: DefList<ItemDef>,
    pub plants: DefList<PlantDef>,
    pub buildings: DefList<BuildingDef>,
    pub recipes: DefList<RecipeDef>,
    pub skills: DefList<SkillDef>,
    pub work_types: DefList<WorkTypeDef>,
    pub stats: DefList<StatDef>,
    pub needs: DefList<NeedDef>,
    pub races: DefList<RaceDef>,
    pub quirks: DefList<QuirkDef>,
    pub thoughts: DefList<ThoughtDef>,
    pub core: CoreIds,
    names: Rc<NameTables>,
}

#[derive(Deserialize)]
struct NameOnly {
    name: String,
}

/// Source of def files: kind -> list of (file label, RON text), in load order.
pub type DefSources = Vec<(&'static str, Vec<(String, String)>)>;

const KINDS: [&str; 12] = [
    "terrain",
    "items",
    "plants",
    "buildings",
    "recipes",
    "skills",
    "work_types",
    "stats",
    "needs",
    "races",
    "quirks",
    "thoughts",
];

impl Defs {
    pub fn name_tables(&self) -> Rc<NameTables> {
        self.names.clone()
    }

    /// Reads `<dir>/<kind>/*.ron` (sorted by file name) for every kind.
    pub fn read_dir(dir: &Path) -> Result<DefSources, String> {
        let mut out = Vec::new();
        for kind in KINDS {
            let kdir = dir.join(kind);
            let mut files = Vec::new();
            if kdir.is_dir() {
                let mut paths: Vec<_> = std::fs::read_dir(&kdir)
                    .map_err(|e| format!("{}: {e}", kdir.display()))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "ron"))
                    .collect();
                paths.sort();
                for p in paths {
                    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    files.push((p.display().to_string(), text));
                }
            }
            out.push((kind, files));
        }
        Ok(out)
    }

    pub fn load_dir(dir: &Path, previous: Option<&Defs>) -> Result<Defs, String> {
        Self::from_sources(&Self::read_dir(dir)?, previous)
    }

    /// Two-pass load: collect every def name, then parse full defs with the
    /// name tables installed so cross-references resolve. With `previous`,
    /// existing names keep their ids (hot reload) and removed defs are kept.
    pub fn from_sources(sources: &DefSources, previous: Option<&Defs>) -> Result<Defs, String> {
        let opts = ron::Options::default();
        let mut tables = NameTables::default();
        for (kind, files) in sources {
            let mut names: Vec<String> = previous.map(|p| p.names_of(kind)).unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for (label, text) in files {
                let list: Vec<NameOnly> = opts.from_str(text).map_err(|e| format!("{label}: {e}"))?;
                for n in list {
                    if !seen.insert(n.name.clone()) {
                        return Err(format!("{label}: duplicate {kind} `{}`", n.name));
                    }
                    if !names.contains(&n.name) {
                        names.push(n.name);
                    }
                }
            }
            tables.add(kind, names);
        }
        let tables = Rc::new(tables);

        let defs = with_names(tables.clone(), || -> Result<Defs, String> {
            fn parse<T: Def + for<'de> Deserialize<'de>>(
                sources: &DefSources,
                tables: &NameTables,
                prev: Option<&DefList<T>>,
            ) -> Result<DefList<T>, String> {
                let opts = ron::Options::default();
                let names = &tables.kinds[T::KIND].names;
                let mut slots: Vec<Option<T>> = vec![None; names.len()];
                let files = &sources.iter().find(|(k, _)| *k == T::KIND).unwrap().1;
                for (label, text) in files {
                    let list: Vec<T> = opts.from_str(text).map_err(|e| format!("{label}: {e}"))?;
                    for d in list {
                        let i = tables.kinds[T::KIND].index[d.name()] as usize;
                        slots[i] = Some(d);
                    }
                }
                let mut items = Vec::with_capacity(slots.len());
                for (i, s) in slots.into_iter().enumerate() {
                    match s.or_else(|| prev.and_then(|p| p.items.get(i).cloned())) {
                        Some(d) => items.push(d),
                        None => return Err(format!("{} `{}` vanished", T::KIND, names[i])),
                    }
                }
                Ok(DefList { items })
            }
            let t = &tables;
            let terrain = parse(sources, t, previous.map(|p| &p.terrain))?;
            let items = parse(sources, t, previous.map(|p| &p.items))?;
            let plants = parse(sources, t, previous.map(|p| &p.plants))?;
            let buildings = parse(sources, t, previous.map(|p| &p.buildings))?;
            let recipes = parse(sources, t, previous.map(|p| &p.recipes))?;
            let skills = parse(sources, t, previous.map(|p| &p.skills))?;
            let work_types = parse(sources, t, previous.map(|p| &p.work_types))?;
            let stats = parse(sources, t, previous.map(|p| &p.stats))?;
            let needs = parse(sources, t, previous.map(|p| &p.needs))?;
            let races = parse(sources, t, previous.map(|p| &p.races))?;
            let quirks = parse(sources, t, previous.map(|p| &p.quirks))?;
            let thoughts = parse(sources, t, previous.map(|p| &p.thoughts))?;

            fn req<T>(r: Option<DefId<T>>, what: &str) -> Result<DefId<T>, String> {
                r.ok_or_else(|| format!("required def `{what}` missing"))
            }
            let core = CoreIds {
                stat_move_speed: req(stats.find("move_speed"), "stats/move_speed")?,
                stat_work_speed: req(stats.find("work_speed"), "stats/work_speed")?,
                stat_mining_speed: req(stats.find("mining_speed"), "stats/mining_speed")?,
                stat_construction_speed: req(stats.find("construction_speed"), "stats/construction_speed")?,
                stat_plant_speed: req(stats.find("plant_speed"), "stats/plant_speed")?,
                stat_learn_rate: req(stats.find("learn_rate"), "stats/learn_rate")?,
                stat_break_threshold: req(stats.find("break_threshold"), "stats/break_threshold")?,
                skill_mining: req(skills.find("mining"), "skills/mining")?,
                skill_construction: req(skills.find("construction"), "skills/construction")?,
                skill_botany: req(skills.find("botany"), "skills/botany")?,
                wt_hauling: req(work_types.find("hauling"), "work_types/hauling")?,
                wt_mining: req(work_types.find("mining"), "work_types/mining")?,
                wt_construction: req(work_types.find("construction"), "work_types/construction")?,
                wt_plants: req(work_types.find("plants"), "work_types/plants")?,
                stairs: req(buildings.find("stairs"), "buildings/stairs")?,
                thought_slept_ground: req(thoughts.find("slept_on_ground"), "thoughts/slept_on_ground")?,
                thought_ate_without_table: thoughts.find("ate_without_table"),
            };
            Ok(Defs {
                terrain,
                items,
                plants,
                buildings,
                recipes,
                skills,
                work_types,
                stats,
                needs,
                races,
                quirks,
                thoughts,
                core,
                names: tables.clone(),
            })
        })?;
        Ok(defs)
    }

    fn names_of(&self, kind: &str) -> Vec<String> {
        self.names.kinds.get(kind).map(|k| k.names.clone()).unwrap_or_default()
    }
}
