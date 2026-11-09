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
pub type EnvFieldId = DefId<EnvFieldDef>;
pub type RoomRoleId = DefId<RoomRoleDef>;
pub type WeatherId = DefId<WeatherDef>;
pub type CapacityId = DefId<CapacityDef>;
pub type BodyId = DefId<BodyDef>;
pub type HediffId = DefId<HediffDef>;
pub type DamageTypeId = DefId<DamageTypeDef>;
pub type InteractionId = DefId<InteractionDef>;

/// Relationship between two colonists (stored symmetrically; Parent/Child
/// mirror each other).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelKind {
    Friend,
    Rival,
    Lover,
    Partner,
    Parent,
    Child,
    Sibling,
}

impl RelKind {
    pub fn label(self) -> &'static str {
        match self {
            RelKind::Friend => "Friend",
            RelKind::Rival => "Rival",
            RelKind::Lover => "Lover",
            RelKind::Partner => "Partner",
            RelKind::Parent => "Parent",
            RelKind::Child => "Child",
            RelKind::Sibling => "Sibling",
        }
    }
    /// The relation as seen from the other side.
    pub fn mirror(self) -> RelKind {
        match self {
            RelKind::Parent => RelKind::Child,
            RelKind::Child => RelKind::Parent,
            k => k,
        }
    }
    pub fn is_family(self) -> bool {
        matches!(self, RelKind::Parent | RelKind::Child | RelKind::Sibling)
    }
    pub fn is_romantic(self) -> bool {
        matches!(self, RelKind::Lover | RelKind::Partner)
    }
}

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
    /// Environment field value at the pawn's cell.
    FieldBelow(EnvFieldId, f32),
    FieldAbove(EnvFieldId, f32),
    /// Aggregate of the field over the pawn's room (false outdoors).
    RoomFieldBelow(EnvFieldId, f32),
    RoomFieldAbove(EnvFieldId, f32),
    /// Field at the pawn's cell is outside the race's comfort range.
    TooLow(EnvFieldId),
    TooHigh(EnvFieldId),
    /// In an enclosed, roofed room.
    Indoors,
    InRoomRole(RoomRoleId),
    ImpressivenessAbove(f32),
    ImpressivenessBelow(f32),
    Weather(WeatherId),
    /// Has a health condition (any severity).
    HasHediff(HediffId),
    /// Total pain above this (0..1).
    PainAbove(f32),
    CapacityBelow(CapacityId, f32),
    Downed,
    /// Lying in a bed (sleeping or resting as a patient).
    InBed,
    /// A corpse lies within this many cells (same level).
    NearCorpse(u32),
    /// Someone with this relation is within this many cells (same level).
    NearRelation(RelKind, u32),
    HasRelation(RelKind),
    /// Sitting on a chair next to a table.
    AtTable,
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
    /// Worldgen grows wild plants on fertile blocks.
    #[serde(default)]
    pub fertile: bool,
    /// Soil fertility of this block when it is the floor under a cell
    /// (crop growth multiplier; 0 = barren).
    #[serde(default)]
    pub fertility: f32,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemCategory {
    #[default]
    Resource,
    RawFood,
    Meal,
    Manufactured,
    /// Sown by farmers; never eaten.
    Seed,
    /// Used by doctors when tending.
    Medicine,
    /// Dead colonists; buried in graves, never stockpiled.
    Corpse,
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
    #[serde(default)]
    pub env: Vec<EnvEmit>,
    /// Usable by doctors when tending.
    #[serde(default)]
    pub medicine: Option<MedicineDef>,
    /// Chance per eaten portion of food poisoning (scaled by the cook's skill
    /// for meals, by filth at the eater's cell).
    #[serde(default)]
    pub poison_chance: f32,
}

/// A treatment source. Potency multiplies tend quality; `max_quality` caps it.
/// Healing magic (salves, spells) plugs in through the same fields.
#[derive(Deserialize, Clone, Debug)]
pub struct MedicineDef {
    pub potency: f32,
    #[serde(default = "one")]
    pub max_quality: f32,
    /// Extra healing-rate multiplier on wounds it tended.
    #[serde(default = "one")]
    pub heal_mult: f32,
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
    /// The plant is removed on harvest (annual crops) instead of regrowing.
    #[serde(default)]
    pub remove: bool,
}

/// Makes a plant sowable in growing zones.
#[derive(Deserialize, Clone, Debug)]
pub struct CropDef {
    /// Seed item consumed per sown cell.
    pub seed: ItemId,
    #[serde(default = "default_sow_work")]
    pub sow_work: f32,
    /// Lowest floor fertility the crop can be sown on.
    #[serde(default = "half")]
    pub min_fertility: f32,
    /// How much fertility matters: growth x lerp(1, fertility, weight).
    #[serde(default = "one")]
    pub fertility_weight: f32,
}

fn default_sow_work() -> f32 {
    60.0
}

/// Growth multiplier from an environment field at the plant's cell:
/// piecewise-linear `curve` of (field value, multiplier) points, clamped at
/// the ends.
#[derive(Deserialize, Clone, Debug)]
pub struct GrowthFactor {
    pub field: EnvFieldId,
    pub curve: Vec<(f32, f32)>,
}

impl GrowthFactor {
    pub fn eval(&self, v: f32) -> f32 {
        eval_curve(&self.curve, v)
    }
}

pub fn eval_curve(curve: &[(f32, f32)], v: f32) -> f32 {
    let (Some(first), Some(last)) = (curve.first(), curve.last()) else { return 1.0 };
    if v <= first.0 {
        return first.1;
    }
    if v >= last.0 {
        return last.1;
    }
    for w in curve.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if v <= x1 {
            let t = if x1 > x0 { (v - x0) / (x1 - x0) } else { 1.0 };
            return y0 + (y1 - y0) * t;
        }
    }
    last.1
}

/// Health damage while a field is out of bounds (frost, heat, ...).
#[derive(Deserialize, Clone, Debug)]
pub struct PlantHarm {
    pub field: EnvFieldId,
    #[serde(default)]
    pub below: Option<f32>,
    #[serde(default)]
    pub above: Option<f32>,
    /// Health (0..1) lost per day while it applies.
    pub per_day: f32,
    /// Shown in the inspector and the death message ("frost").
    #[serde(default)]
    pub label: String,
}

impl PlantHarm {
    pub fn applies(&self, v: f32) -> bool {
        self.below.is_some_and(|b| v < b) || self.above.is_some_and(|a| v > a)
    }
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
    #[serde(default)]
    pub env: Vec<EnvEmit>,
    /// Sowable in growing zones.
    #[serde(default)]
    pub crop: Option<CropDef>,
    /// Growth-rate multipliers from environment fields.
    #[serde(default)]
    pub growth: Vec<GrowthFactor>,
    /// Conditions that damage the plant's health.
    #[serde(default)]
    pub harm: Vec<PlantHarm>,
    /// Health regained per day while unharmed.
    #[serde(default = "default_heal")]
    pub heal_per_day: f32,
}

fn default_heal() -> f32 {
    0.3
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingKind {
    Wall,
    Door,
    Bed,
    Workbench,
    Stairs,
    Furniture,
    /// Holds one buried corpse.
    Grave,
    /// Sit-down furniture: tables and chairs (social meals).
    Table,
    Chair,
    /// Two-player recreation (dice, chess...).
    GameTable,
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
    /// Sleeping places (2 = double bed shared by partners).
    #[serde(default = "one_u32")]
    pub slots: u32,
    /// Rest gain multiplier when sleeping here (beds).
    #[serde(default = "one")]
    pub rest_factor: f32,
    #[serde(default)]
    pub sleep_thought: Option<ThoughtId>,
    /// Healing-rate multiplier for patients resting here (beds).
    #[serde(default = "one")]
    pub heal_factor: f32,
    /// Doctors prefer these beds for patients.
    #[serde(default)]
    pub medical: bool,
    #[serde(default)]
    pub light: Option<LightDef>,
    #[serde(default)]
    pub env: Vec<EnvEmit>,
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
    /// Anatomy (health model).
    pub body: BodyId,
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
    /// Comfortable ranges for environment fields (see `TooLow` / `TooHigh`).
    #[serde(default)]
    pub comfort: Vec<(EnvFieldId, f32, f32)>,
    /// Starting opinion offset toward colonists of other races; fades as
    /// they get to know each other.
    #[serde(default)]
    pub attitudes: Vec<(RaceId, f32)>,
    /// Romantic compatibility with races (own race defaults to 1, others to
    /// `romance_default`).
    #[serde(default)]
    pub romance: Vec<(RaceId, f32)>,
    #[serde(default = "default_romance")]
    pub romance_default: f32,
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

// -------------------------------------------------------------- environment

/// How a field changes over time.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldKind {
    /// Stateful per-cell value: emitters add to it, it diffuses between open
    /// cells, and relaxes toward ambient (temperature, filth, ...).
    #[default]
    Dynamic,
    /// Recomputed from emitters and inputs each update (beauty, light, ...).
    Static,
}

/// Ambient (outdoor) baseline of a field.
#[derive(Deserialize, Clone, Copy, Debug)]
pub enum Ambient {
    Constant(f32),
    /// mean, seasonal amplitude, daily amplitude (warmest mid-summer / mid-afternoon).
    Climate(f32, f32, f32),
}

impl Default for Ambient {
    fn default() -> Self {
        Ambient::Constant(0.0)
    }
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub enum FieldInput {
    /// Another field's value at the same cell.
    Field(EnvFieldId),
    /// Outdoor daylight 0..1 (0 when roofed), scaled by weather.
    Daylight,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoomAgg {
    #[default]
    Average,
    Sum,
    Min,
    Max,
}

#[derive(Deserialize, Clone, Debug)]
pub struct EnvFieldDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub decimals: u8,
    #[serde(default)]
    pub kind: FieldKind,
    /// Value of a fresh cell / static baseline.
    #[serde(default)]
    pub default: f32,
    #[serde(default)]
    pub min: Option<f32>,
    #[serde(default)]
    pub max: Option<f32>,
    // --- dynamic fields
    /// Fraction exchanged with each open neighbour per update (0..0.2).
    #[serde(default)]
    pub diffusion: f32,
    /// Fraction pulled toward ambient per update when not roofed / roofed.
    #[serde(default)]
    pub outdoor_pull: f32,
    #[serde(default)]
    pub indoor_pull: f32,
    #[serde(default)]
    pub ambient: Ambient,
    /// Solid neighbours act as a constant boundary at this value
    /// (`None` = they are ignored). Climate fields default to their mean.
    #[serde(default)]
    pub ground: Option<f32>,
    /// Added to a cell each time a pawn steps onto it.
    #[serde(default)]
    pub traffic: f32,
    #[serde(default)]
    pub traffic_indoor_only: bool,
    // --- static fields
    #[serde(default)]
    pub inputs: Vec<(FieldInput, f32)>,
    /// Emitters only count for cells in the same room (or both outdoors).
    #[serde(default)]
    pub same_room_only: bool,
    // --- rooms & UI
    #[serde(default)]
    pub room_agg: RoomAgg,
    /// Contribution of the room aggregate to room impressiveness.
    #[serde(default)]
    pub impressiveness: f32,
    /// Overlay colour range (low, high).
    #[serde(default)]
    pub overlay: Option<(f32, f32)>,
}

impl EnvFieldDef {
    pub fn clamp(&self, v: f32) -> f32 {
        let v = self.min.map_or(v, |m| v.max(m));
        self.max.map_or(v, |m| v.min(m))
    }

    pub fn format(&self, v: f32) -> String {
        format!("{:.*}{}", self.decimals as usize, v, self.unit)
    }
}

/// Something that affects an environment field around a thing.
#[derive(Deserialize, Clone, Debug)]
pub struct EnvEmit {
    pub field: EnvFieldId,
    /// Static fields: value added at full strength. Dynamic fields: amount
    /// added per update (or pull rate toward `target`).
    pub amount: f32,
    /// Reach in cells (0 = own cell only); strength falls off linearly.
    #[serde(default)]
    pub radius: f32,
    /// Dynamic fields: push toward this value instead of adding (thermostats).
    #[serde(default)]
    pub target: Option<f32>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RoomRoleDef {
    pub name: String,
    pub label: String,
    /// Buildings that count toward this role.
    pub buildings: Vec<BuildingId>,
    #[serde(default = "one_u32")]
    pub min_count: u32,
    #[serde(default)]
    pub max_count: Option<u32>,
    /// Highest-priority matching role wins.
    #[serde(default)]
    pub priority: f32,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeatherVisual {
    #[default]
    Clear,
    Rain,
    Fog,
    AetherStorm,
}

#[derive(Deserialize, Clone, Debug)]
pub struct WeatherDef {
    pub name: String,
    pub label: String,
    #[serde(default = "one")]
    pub weight: f32,
    /// Duration range in hours.
    pub hours: (f32, f32),
    /// Offsets added to fields' ambient values.
    #[serde(default)]
    pub ambient: Vec<(EnvFieldId, f32)>,
    /// Multiplier on outdoor daylight.
    #[serde(default = "one")]
    pub light: f32,
    #[serde(default)]
    pub visual: WeatherVisual,
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

fn default_romance() -> f32 {
    0.3
}

// ------------------------------------------------------------------- health

/// A bodily ability (moving, manipulation...), 0..1 (1 = healthy).
#[derive(Deserialize, Clone, Debug)]
pub struct CapacityDef {
    pub name: String,
    pub label: String,
    /// Death when it reaches zero.
    #[serde(default)]
    pub vital: bool,
    /// Downed while below this.
    #[serde(default)]
    pub downed_below: Option<f32>,
    /// Multiplied by lerp(1, other, weight) for each entry (earlier
    /// capacities only), e.g. moving depends on consciousness.
    #[serde(default)]
    pub depends: Vec<(CapacityId, f32)>,
    /// Offset per unit of total pain (consciousness: negative).
    #[serde(default)]
    pub pain: f32,
    /// Stats multiplied by lerp(1, value, weight).
    #[serde(default)]
    pub stats: Vec<(StatId, f32)>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BodyPartDef {
    pub name: String,
    pub label: String,
    /// Parent part name (must appear earlier in the list); None for the root.
    #[serde(default)]
    pub parent: Option<String>,
    pub hp: f32,
    /// Chance a hit on the parent lands on this part instead (0..1).
    #[serde(default)]
    pub coverage: f32,
    /// Internal organ: only struck by hits that get through the parent.
    #[serde(default)]
    pub inside: bool,
    /// Free tags: `vital` (blunt non-lethal damage spares it), `extremity`
    /// (frostbite), ...
    #[serde(default)]
    pub tags: Vec<String>,
    /// Capacities this part contributes to, with weight.
    #[serde(default)]
    pub capacities: Vec<(CapacityId, f32)>,
    /// Destroying it kills outright (brain, heart).
    #[serde(default)]
    pub kills: bool,
    /// Capacities multiplied by this part's efficiency (spine → moving).
    #[serde(default)]
    pub critical_for: Vec<CapacityId>,
    /// Rectangle on the body diagram (x, y, w, h) in diagram pixels.
    #[serde(default)]
    pub diagram: Option<(u8, u8, u8, u8)>,
    #[serde(skip)]
    pub parent_idx: Option<usize>,
}

impl BodyPartDef {
    pub fn has_tag(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct BodyDef {
    pub name: String,
    pub label: String,
    pub parts: Vec<BodyPartDef>,
    /// Downed by pain shock above this total pain.
    #[serde(default = "default_pain_shock")]
    pub pain_shock: f32,
    /// Diagram size in pixels (w, h).
    #[serde(default)]
    pub diagram_size: (u8, u8),
}

fn default_pain_shock() -> f32 {
    0.8
}

impl BodyDef {
    pub fn part(&self, name: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.name == name)
    }
    /// Children of a part, in def order.
    pub fn children(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        self.parts.iter().enumerate().filter(move |(_, p)| p.parent_idx == Some(idx)).map(|(i, _)| i)
    }
    pub fn is_ancestor(&self, anc: usize, mut idx: usize) -> bool {
        while let Some(p) = self.parts[idx].parent_idx {
            if p == anc {
                return true;
            }
            idx = p;
        }
        false
    }
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HediffKind {
    /// Damage on a part; severity = HP lost.
    #[default]
    Injury,
    /// A destroyed or lost part (it and its children stop working).
    MissingPart,
    /// Permanent leftover of an injury.
    Scar,
    /// Disease racing against immunity.
    Illness,
    /// Whole-body state (blood loss, hypothermia, food poisoning...).
    Condition,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct HediffStage {
    /// Applies from this severity up.
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub pain: f32,
    /// Capacity offsets.
    #[serde(default)]
    pub capacities: Vec<(CapacityId, f32)>,
    #[serde(default)]
    pub stat_mods: Vec<StatMod>,
    /// While in this stage, adds this injury to a random part with the tag,
    /// with the given chance per day (frostbite from hypothermia).
    #[serde(default)]
    pub spawns: Option<(HediffId, String, f32)>,
    /// Severity of a spawned injury.
    #[serde(default)]
    pub spawn_severity: f32,
}

/// Disease progression: severity rises until immunity reaches 1.
#[derive(Deserialize, Clone, Debug)]
pub struct ImmunityDef {
    pub severity_per_day: f32,
    /// Multiplier on severity gain at tend quality 1 (lerped from 1).
    #[serde(default = "half")]
    pub tended_factor: f32,
    pub immunity_per_day: f32,
    /// Severity lost per day once immune.
    pub recover_per_day: f32,
}

/// Exposure conditions: while `when` holds, severity changes by `curve` over
/// a field at the pawn's cell (if given) or by `gain_per_day`; otherwise it
/// falls by `loss_per_day`. Negative curve values heal.
#[derive(Deserialize, Clone, Debug)]
pub struct HediffTrigger {
    #[serde(default)]
    pub when: Condition,
    #[serde(default)]
    pub curve: Option<(EnvFieldId, Vec<(f32, f32)>)>,
    #[serde(default)]
    pub gain_per_day: f32,
    pub loss_per_day: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct HediffDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub kind: HediffKind,
    /// Blood loss per day per severity point while untended.
    #[serde(default)]
    pub bleed: f32,
    /// Pain per severity point (injuries) or flat (others, scaled by severity).
    #[serde(default)]
    pub pain: f32,
    /// Severity lost per day naturally.
    #[serde(default)]
    pub heal_per_day: f32,
    /// Healing multiplier at tend quality 1 (lerped from 1).
    #[serde(default = "default_tended_heal")]
    pub tended_heal: f32,
    #[serde(default)]
    pub tendable: bool,
    /// Hours a tend lasts (illnesses need re-tending).
    #[serde(default)]
    pub tend_hours: Option<f32>,
    /// (infection hediff, chance) rolled once when the wound is 12 h old;
    /// reduced by tend quality, raised by filth.
    #[serde(default)]
    pub infection: Option<(HediffId, f32)>,
    /// Chance a healed injury leaves a scar.
    #[serde(default)]
    pub scar_chance: f32,
    #[serde(default)]
    pub stages: Vec<HediffStage>,
    /// Death at this severity.
    #[serde(default)]
    pub lethal_at: Option<f32>,
    #[serde(default)]
    pub max_severity: Option<f32>,
    #[serde(default)]
    pub immunity: Option<ImmunityDef>,
    #[serde(default)]
    pub trigger: Option<HediffTrigger>,
    /// Random illness events: weight in the illness pool (0 = never).
    #[serde(default)]
    pub event_weight: f32,
    /// Starting severity for events / spawns.
    #[serde(default)]
    pub initial: f32,
    /// Shown in the health tab even at zero pain.
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_tended_heal() -> f32 {
    2.0
}

impl HediffDef {
    /// The highest stage whose `min` is at or below `sev`.
    pub fn stage(&self, sev: f32) -> Option<&HediffStage> {
        self.stages.iter().rev().find(|s| sev >= s.min)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct DamageTypeDef {
    pub name: String,
    pub label: String,
    /// Injury hediff it causes.
    pub injury: HediffId,
    /// Can destroy parts (else HP stops at 1).
    #[serde(default = "default_true")]
    pub destroys: bool,
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
    /// Multiplies race attitudes (Bigot > 1, Open-minded 0).
    #[serde(default = "one")]
    pub race_bias: f32,
    /// Multiplies how often this colonist starts conversations.
    #[serde(default = "one")]
    pub chattiness: f32,
    /// Multiplies the chance an insult turns into a fight.
    #[serde(default = "one")]
    pub fight_chance: f32,
}

/// A social interaction between two nearby colonists.
#[derive(Deserialize, Clone, Debug)]
pub struct InteractionDef {
    pub name: String,
    pub label: String,
    /// Speech-bubble symbol drawn over the initiator.
    #[serde(default)]
    pub icon: String,
    #[serde(default = "one")]
    pub weight: f32,
    /// Weight multiplier by the initiator's opinion of the recipient.
    #[serde(default)]
    pub opinion_curve: Vec<(f32, f32)>,
    /// Weight multipliers from the initiator's quirks.
    #[serde(default)]
    pub quirk_weights: Vec<(QuirkId, f32)>,
    /// Opinion change of the recipient toward the initiator (scaled by the
    /// initiator's social impact) and of the initiator toward the recipient.
    #[serde(default)]
    pub opinion: f32,
    #[serde(default)]
    pub opinion_initiator: f32,
    /// How long the opinion memory lasts (days).
    #[serde(default = "default_memory_days")]
    pub memory_days: f32,
    /// Memory thoughts given to the recipient / initiator.
    #[serde(default)]
    pub thought: Option<ThoughtId>,
    #[serde(default)]
    pub thought_initiator: Option<ThoughtId>,
    /// Romantic advance (may start a relationship).
    #[serde(default)]
    pub romance: bool,
    /// May escalate into a social fight.
    #[serde(default)]
    pub insult: bool,
    /// Fights it starts can kill (no blunt-damage safeguards).
    #[serde(default)]
    pub lethal_fights: bool,
    /// Social XP for the initiator.
    #[serde(default = "default_social_xp")]
    pub xp: f32,
    /// Log line ("{a} insulted {b}."); empty = not logged.
    #[serde(default)]
    pub log: String,
}

fn default_memory_days() -> f32 {
    3.0
}
fn default_social_xp() -> f32 {
    40.0
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
    EnvFieldDef => "env_fields",
    RoomRoleDef => "room_roles",
    WeatherDef => "weather",
    CapacityDef => "capacities",
    BodyDef => "bodies",
    HediffDef => "hediffs",
    DamageTypeDef => "damage_types",
    InteractionDef => "interactions",
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
    pub wt_cleaning: Option<WorkTypeId>,
    pub wt_doctor: Option<WorkTypeId>,
    pub field_filth: Option<EnvFieldId>,
    pub field_temperature: Option<EnvFieldId>,
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
    pub env_fields: DefList<EnvFieldDef>,
    pub room_roles: DefList<RoomRoleDef>,
    pub weather: DefList<WeatherDef>,
    pub capacities: DefList<CapacityDef>,
    pub bodies: DefList<BodyDef>,
    pub hediffs: DefList<HediffDef>,
    pub damage_types: DefList<DamageTypeDef>,
    pub interactions: DefList<InteractionDef>,
    pub core: CoreIds,
    names: Rc<NameTables>,
}

#[derive(Deserialize)]
struct NameOnly {
    name: String,
}

/// Source of def files: kind -> list of (file label, RON text), in load order.
pub type DefSources = Vec<(&'static str, Vec<(String, String)>)>;

const KINDS: [&str; 20] = [
    "interactions",
    "capacities",
    "bodies",
    "hediffs",
    "damage_types",
    "env_fields",
    "room_roles",
    "weather",
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
            let env_fields = parse(sources, t, previous.map(|p| &p.env_fields))?;
            let room_roles = parse(sources, t, previous.map(|p| &p.room_roles))?;
            let weather = parse(sources, t, previous.map(|p| &p.weather))?;
            let capacities = parse(sources, t, previous.map(|p| &p.capacities))?;
            let mut bodies = parse(sources, t, previous.map(|p| &p.bodies))?;
            let hediffs = parse(sources, t, previous.map(|p| &p.hediffs))?;
            let damage_types = parse(sources, t, previous.map(|p| &p.damage_types))?;
            let interactions = parse(sources, t, previous.map(|p| &p.interactions))?;
            resolve_bodies(&mut bodies)?;
            for (id, c) in capacities.iter() {
                if c.depends.iter().any(|(d, _)| d.index() >= id.index()) {
                    return Err(format!("capacity `{}` depends on a later capacity", c.name));
                }
            }

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
                wt_cleaning: work_types.find("cleaning"),
                wt_doctor: work_types.find("doctor"),
                field_filth: env_fields.find("filth"),
                field_temperature: env_fields.find("temperature"),
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
                env_fields,
                room_roles,
                weather,
                capacities,
                bodies,
                hediffs,
                damage_types,
                interactions,
                core,
                names: tables.clone(),
            })
        })?;
        Ok(defs)
    }

    /// Health ids code refers to directly.
    pub fn health_ids(&self) -> HealthIds {
        HealthIds {
            blood_loss: self.hediffs.find("blood_loss"),
            missing: self.hediffs.find("missing_part"),
            scar: self.hediffs.find("scar"),
            cap_consciousness: self.capacities.find("consciousness"),
            cap_moving: self.capacities.find("moving"),
            cap_manipulation: self.capacities.find("manipulation"),
        }
    }

    fn names_of(&self, kind: &str) -> Vec<String> {
        self.names.kinds.get(kind).map(|k| k.names.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HealthIds {
    pub blood_loss: Option<HediffId>,
    pub missing: Option<HediffId>,
    pub scar: Option<HediffId>,
    pub cap_consciousness: Option<CapacityId>,
    pub cap_moving: Option<CapacityId>,
    pub cap_manipulation: Option<CapacityId>,
}

/// Links body parts to their parents (by name) and checks the tree.
fn resolve_bodies(bodies: &mut DefList<BodyDef>) -> Result<(), String> {
    for b in &mut bodies.items {
        let names: Vec<String> = b.parts.iter().map(|p| p.name.clone()).collect();
        if names.is_empty() {
            return Err(format!("body `{}` has no parts", b.name));
        }
        if b.parts.len() > 250 {
            return Err(format!("body `{}` has too many parts", b.name));
        }
        for (i, p) in b.parts.iter_mut().enumerate() {
            p.parent_idx = match &p.parent {
                None if i == 0 => None,
                None => return Err(format!("body `{}`: part `{}` has no parent", b.name, p.name)),
                Some(n) => match names.iter().position(|x| x == n) {
                    Some(j) if j < i => Some(j),
                    _ => return Err(format!("body `{}`: part `{}` has unknown or later parent `{n}`", b.name, p.name)),
                },
            };
        }
    }
    Ok(())
}
