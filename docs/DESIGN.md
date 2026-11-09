# Colony Game — Design Document

> Working title. Living document: update it when decisions change.

## 1. Vision

An isometric, pixel-art colony simulator in the tradition of RimWorld and Dwarf
Fortress, set in a **sci-fi post-apocalyptic fantasy** world. Fantasy peoples
(humans, elves, dwarves, orcs, beastfolk...) scrape out a living among the ruins
of a high-tech civilisation that ended in an arcane catastrophe. Old-world
technology and resurgent magic are *both* real, distinct disciplines — and the
most powerful things come from weaving them together.

### Pillars

1. **Indirect control.** The player never drives a colonist. They designate
   work, set priorities, queue production and lay out the colony; autonomous
   colonists do the rest.
2. **People, not units.** Colonists have races with genuinely different needs,
   randomised quirks that change how well (and how happily) they work, skills
   that grow with use, and moods that can break.
3. **Survival first, industry later.** Early game is raw gathering and
   hand-crafting. Later game adds production chains, robots and factories —
   *light* automation, never a pure factory game.
4. **Systems over scripts.** Content is data; behaviour emerges from
   interacting systems. Determinism makes everything reproducible.

## 2. Setting

### 2.1 The world

Centuries ago a technological civilisation tapped the *Aether* — the substrate
that magic flows through — as a power source. The **Sundering** tore it open.
Cities burned, the sky still carries aether storms, and ruins full of salvage
dot the land. Magic, long dormant, came back with the fallout.

### 2.2 Magic and technology

Two distinct disciplines that **intertwine** rather than one being late-game
unlock of the other:

| | Technology | Magic |
|---|---|---|
| Skill | Engineering | Arcana |
| Raw inputs | Scrap, components, ore, fuel | Aether shards, reagents, attuned materials |
| Energy | Electricity (generators, batteries) | Mana (ley lines, crystals, rituals) |
| Strength | Reliable, mass-producible, automatable | Flexible, powerful, personal, risky |
| Weakness | Needs supply chains & power | Unstable, needs talent, can backfire |

Intertwining examples (roadmap, not M1): aether-cell generators (magic → power),
runic machining (tech workstation that inscribes enchantments), golem-robots
(robot chassis + bound spirit), arcane sensors on turrets. Systems are designed
generically (resource types, energy networks, effects) so the lore can flex.

### 2.3 Peoples (races)

Races are **deeply distinct**: different need sets, metabolism, lifespans, skill
aptitudes, quirk pools and body plans.

| Race | Concept | Distinguishing mechanics (draft) | Milestone |
|---|---|---|---|
| Humans | Adaptable survivors | Faster learning; need Recreation | M1 |
| Elves | Long-lived, attuned | Need Nature (outdoors, near plants); slower rest decay; Arcana aptitude; dislike underground | M1 |
| Dwarves | Stone-folk engineers | Need Industry (satisfied by doing work); like underground; Mining/Crafting aptitude; slow | M1 |
| Synthetics | Old-world androids | Power + maintenance instead of food/sleep; rigid personalities; bridge to robots | After power/automation systems exist (M3+) |
| Mutants (Wastefolk) | Humans changed by aether fallout | Aether/radiation resistant; unstable mutation quirks; shunned by others (social prejudice) | M2 |
| Lupine | Wolf-folk (Beastfolk) | Pack instincts: need company, strong social bonds; hunting/combat aptitude | M2 |
| Ursine | Bear-folk (Beastfolk) | Huge, strong, slow; big appetite; seasonal torpor (sleep more in cold) | M2 |
| Feline | Cat-folk (Beastfolk) | Nimble, night-sighted, independent; dislike being told what to do | M2 |
| Orcs | Scrappers and salvagers | Great at junk tech & salvage, poor at fine crafts | M2 |
| Lizardfolk | Cold-blooded | Temperature-dependent activity | Later |
| Harpies | Avian folk | Need open sky; fast movement (flight later) | Later |

No goblins. Lupine, Ursine and Feline are separate races (not sub-races of one "Beastfolk" def); they may share quirks and thoughts.

## 3. Technical Architecture

Rust (edition 2024), Cargo workspace:

```
crates/
  ecs/      bespoke ECS (entities, sparse-set storages, resources, commands, serialization registry)
  sim/      deterministic, headless game simulation (no graphics deps)
  client/   wgpu + winit renderer, bespoke pixel UI, input, binary `colony`
assets/
  defs/     content definitions (.ron), hot-reloaded in dev
  sprites/  baked sprite sheets + manifest (output of tools/bake)
tools/
  bake/     Blender (headless) scripts that model and render sprites
docs/
```

**Strict layering:** `sim` never depends on `client`. The simulation can run
headless (tests, benchmarks, future dedicated tooling). The client reads sim
state and sends *orders* (designations, bills, priorities) into it.

### 3.1 ECS (`crates/ecs`)

Purpose-built for a single-threaded, deterministic colony sim rather than a
general-purpose engine:

- **Entities:** generational indices (`index: u32, generation: u32`).
- **Storage:** one `SparseSet<T>` per component type. Colony sims add/remove
  components constantly (jobs, tags, carried items, blueprints); sparse sets make
  that O(1) and keep iteration dense. Iteration order is deterministic (it only
  depends on the sequence of operations).
- **Borrowing:** each storage lives in a `RefCell`, so a system can hold
  `write::<Needs>()` and `read::<Position>()` at the same time. Joins are done by
  iterating the smallest set and probing the others.
- **Resources:** singleton typed values (map, clock, RNG, defs, zones...).
- **Commands:** deferred spawn/despawn/insert/remove, applied between systems,
  so systems can mutate structure while iterating.
- **Systems & schedule:** plain `fn(&mut World)` run in an explicit, fixed
  order. No parallel scheduler — determinism and debuggability over raw
  throughput. (Hot spots like pathfinding can be parallelised internally later.)
- **Serialization registry:** components/resources opt in to saving with a
  stable string name; the save stores each storage as a named blob, so unknown
  or removed types are skipped gracefully.

### 3.2 Determinism

- Fixed simulation tick. Nothing in `sim` reads wall-clock time or OS randomness.
- One seeded RNG (`SimRng`, xoshiro256**), stored as a resource and saved.
- No iteration over `HashMap`/`HashSet` in simulation logic (use `Vec`,
  `BTreeMap`, sparse sets, or sorted keys).
- Floating point is allowed; determinism is guaranteed *for the same binary on
  the same platform* (enough for saves, replays and bug repro). Cross-platform
  lockstep would require fixed point — not a goal.
- A `state_hash()` over the world lets tests assert two runs are identical.

### 3.3 Time

- 30 ticks per in-game "simulation second" at 1× speed.
- 1 in-game hour = 1 250 ticks; 1 day = 24 h = 30 000 ticks ≈ 16.7 real minutes at 1×.
- Speeds: paused, 1× (30 tps), 2× (60), 3× (120), 4× (240, dev/ultra).
- The client renders at display rate and interpolates movement.

### 3.4 Content definitions (defs)

All content lives in `assets/defs/*.ron`: terrain, items, plants, buildings,
recipes, races, needs, skills, work types, quirks, thoughts, stats.

- Defs are referenced at runtime by small typed indices (`DefId<ItemDef>`).
- **Saves store def *names***, not indices; loading remaps by name, so editing
  or reordering defs never corrupts saves.
- **Hot reload:** the client watches `assets/defs/` and reloads on change;
  existing names keep their ids, new names are appended. Invalid files are
  reported in-game and the old defs stay active.

### 3.5 Save / load

- Whole-world snapshot: entity allocator + every registered component storage +
  registered resources, each as a named binary blob (bincode), plus a header
  (version, def name tables).
- Quick save/load (F5/F9) in M1. Save format is versioned.

## 4. World & Map

- Single colony map for now (default 128 × 128) with **a few z-levels** (5:
  two underground layers, surface band, and one layer above for hills / upper
  floors). An overworld is a future addition; the map gen takes a seed and biome
  parameters so it can be driven by one later.
- Map is a 3D grid of **cells**. Each cell is either *solid* (a terrain block:
  soil, stone, ore, ruin-rubble) or *open*. An open cell is **standable** if the
  cell below is solid or has a constructed floor.
- Vertical travel: natural **ramps** generated at 1-level terrain steps, and
  constructed **stairs**. Hills can be mined into horizontally; deeper layers are
  reached via stairs.
- Things on the map (trees, items, buildings, pawns) are entities with a
  `Position` (integer cell). Fast spatial lookups via per-cell indices kept in
  the map resource.
- Worldgen (M1): value-noise heightmap → surface level per column; soil over
  stone; ore and *salvage* veins (ruins of the old world: scrap, components);
  trees, berry bushes, grass by fertility noise.

## 5. Rendering

### 5.1 Look

**Crisp pixel art produced from 3D models.** Models are rendered offline in
Blender at low resolution, no anti-aliasing, flat/cel shading with outlines,
then palette-quantised. The client draws them with nearest-neighbour sampling
at integer zoom (1×–4×), so pixels stay crisp.

### 5.2 Projection

- 2:1 dimetric ("pixel isometric"), camera elevation 30°.
- Tile footprint: **48 × 24 px** diamond. One z-level (a cube) is **30 px** tall
  (M1 used 32 × 16 / 20 px; changed in M1.5).
- Four camera angles (azimuth 45°, 135°, 225°, 315°), rotated with Q/E. Rotation
  is a pure coordinate transform of the grid; sprites are chosen per angle.
- Draw order: sort key (rotated depth `x'+y'`, z, layer, sub-order). Pawns use
  interpolated depth while walking.
- **Cutaway:** the player views a z-level; everything above it is hidden and
  solid cells at the view level are drawn as cut tops, so underground levels are
  readable. Lower levels are progressively darkened.
- **Lighting (M1.5):** every sprite has an albedo and a camera-space normal map.
  The shader lights each pixel from the sun (direction follows the time of day
  and is converted to camera space per angle), sky/cave ambient (roofed sprites
  get only cave ambient), and up to 64 point lights from `light` defs on
  buildings. Light is quantised into bands to keep the pixel-art look; emissive
  pixels (flames, screens) are encoded as a zero normal and stay full-bright.

### 5.3 Sprite bake pipeline (`tools/bake`)

1. Models are authored in Blender — initially *procedurally* by Python scripts
   (low-poly from primitives), later by hand in .blend files.
2. `blender -b -P tools/bake/bake.py` renders each model with an orthographic
   camera matching the projection exactly (1 Blender unit = 1 tile).
   - Static objects / blocks: 4 azimuths.
   - Rotatable buildings: 4 azimuths × 4 rotations collapse to 4 sheets (index
     `(rotation + camera) % 4`).
   - Characters: 8 facings × animation frames (idle, walk, work...).
3. Three passes per frame, all via vertex colours with FLAT shading: albedo
   (unlit), normal (camera space), and layer id (colonists only).
4. Post-process: hard alpha threshold, albedo posterisation, *selective*
   outline (darkened neighbour colour), trimming.
5. Output: `atlas.png` + `atlas_n.png` (same layout) + `manifest.ron`
   (sprite → variants × dirs × frames of rects with anchors).

Variants: blocks and plants bake several variants, picked per cell by hash.
Colonists are baked per race × hair style as disjoint pixel layers (skin, top,
bottom, hair, detail) split from one composite by the id pass, so occlusion is
correct and layers can be tinted with each colonist's `Appearance` colours.
Tools are baked per race × tool for the two work frames and drawn on top.
Pawn frames: 0 idle, 1–4 walk, 5 lying, 6–7 work. A new race is mostly a new
parameter set plus palettes in its def.

## 6. UI

A **bespoke pixel UI** drawn by our own renderer (no egui):

- Immediate-mode API (`ui.button(...)`, `ui.label(...)`, panels, windows) over a
  small layout system; bitmap font; nine-slice-ish panel frames in the same
  palette as the world.
- UI captures input first; unhandled input goes to world tools.
- M1 screens: top bar (date, time, speed, z-level, camera angle), bottom
  toolbar (Orders / Zones / Build / Work / Colonists), inspector panel
  (colonist: race, quirks, needs, mood & thoughts, skills, job; bench: bills),
  work-priority grid, hover tooltip, messages log.

## 7. Simulation Systems

### 7.1 Pawns (colonists)

Components: `Pawn` (name, race, age), `Needs`, `Mood`, `Skills`, `Quirks`,
`WorkPriorities`, `Mover`, `CurrentJob`, `Inventory` (carried stack), `MentalState`.

### 7.2 Stats & modifiers

Every proficiency is a **stat** (`MoveSpeed`, `WorkSpeed`, `MiningSpeed`,
`ConstructionSpeed`, `PlantWorkSpeed`, `CookingSpeed`, `CraftingSpeed`,
`CraftQuality`, `HungerRate`, `RestRate`, `MentalBreakThreshold`, ...).

`value = race base × skill curve × Π(quirk/thought/need modifiers)`

Modifiers are data: `StatMod { stat, op: Add | Mul, when: Condition }` where
`Condition` is a small evaluated enum: `Always`, `Underground`, `Outdoors`,
`Night`, `Day`, `Alone`, `NearOthers(r)`, `NearPlants(r)`, `NeedBelow(need, v)`,
`MoodBelow(v)`, `All[..]`, `Any[..]`, `Not(..)`. This is how quirks affect
proficiency situationally ("Night Owl: +20 % work speed at night, −10 % by day").

### 7.3 Needs

Defined in data; each race lists which needs it has and their decay rates.
M1 needs: **Hunger**, **Rest**, **Recreation** (humans), **Nature** (elves),
**Industry** (dwarves). Needs satisfy through jobs (eat, sleep, relax) or
conditions (Nature rises while outdoors near plants; Industry rises while
working). Low needs create thoughts and urgent jobs.

### 7.4 Mood & thoughts

Mood = baseline + Σ active thoughts, clamped 0–100.

- **Situational thoughts** — evaluated periodically from conditions
  ("Hungry", "Exhausted", "Cramped underground").
- **Memory thoughts** — events with durations and stack limits
  ("Ate raw food", "Slept on the ground", "Ate a fine meal").
- Thresholds: minor (35), major (20), extreme (5) → chance per hour to start a
  **mental break**. M1 breaks: *Sulking* (wanders, won't work), *Hiding*
  (goes to bed/room and refuses work), *Food binge* (eats from stockpile).

Colonists are **mostly obedient**: they follow priorities reliably; quirks change
speed/quality/mood, and bad moods bite through breaks.

### 7.5 Skills

Levels 0–20, XP gained by doing the work, scaled by race aptitude and quirks.
Small daily decay at high levels. Skill level drives work speed / quality via
per-skill curves. M1 skills: Mining, Construction, Botany, Cooking, Crafting,
(defined but unused yet: Engineering, Arcana, Medicine, Social, Combat).

### 7.6 Quirks

Randomised at colonist generation (2–4 each), drawn from a weighted pool
filtered by race and mutual exclusions. A quirk carries stat mods, need mods,
thought triggers, and skill aptitude changes. Examples: *Night Owl*,
*Claustrophobic*, *Workaholic*, *Lazy*, *Glutton*, *Green Thumb*, *Clumsy*,
*Perfectionist*, *Tunnel Rat*, *Iron Stomach*, *Light Sleeper*, *Optimist*,
*Pessimist*.

### 7.7 Work & jobs

- **Designations** (map orders): Mine, Chop, Harvest, Cancel, Build (blueprint),
  Stockpile zones.
- **Work types**: Hauling, Mining, Construction, Plants, Cooking, Crafting —
  each with a **work giver** that finds candidate targets.
- **Priorities**: per-colonist grid, 0 (off) / 1 (highest) … 4. Idle pawns pick
  by: urgent needs → mental state → work types ordered by priority, then by
  def order → closest reachable target (flood-fill search) → recreation/wander.
- **Jobs** are sequences of **toils** (`GoTo`, `PickUp`, `Work(n)`, `Drop`,
  `Consume`, `Sleep`, `Wait`...). Targets and items are **reserved** so pawns
  don't collide on the same task. Jobs fail gracefully (path blocked, target
  gone) and release reservations.
- **Pathfinding:** A* over cells (8-way on a level, ramps/stairs between
  levels, no corner-cutting), integer costs. Closest-target searches use
  Dijkstra flood fill.

### 7.8 Resources, items & crafting

- Raw gathering: chop trees → Wood; mine stone → Stone; mine ore/salvage veins
  → Iron Ore / Scrap / Old Components; harvest berry bushes → Berries.
- Items are stacks on the ground (one stack per cell); hauling moves them to
  **stockpile zones** with item filters.
- Workbenches hold **bills** (recipe + count or "forever"); a crafter gathers
  ingredients, works, and produces output. Quality from skill + quirks.
- M1 production: *Campfire* (Berries → Simple Meal), *Workbench* (Scrap → Metal
  Parts; Wood → Wooden Tools), *Stonecutter* (Stone → Stone Blocks).
- Construction: blueprints need materials hauled in, then construction work.
  M1 buildables: Wall (wood/stone), Door, Bed, Campfire, Tinker's bench,
  Stonecutter. Stairs come from digging down.

### 7.9 Automation (roadmap)

Light automation only: buildable **robots** (Synthetic tech) that take simple
work types (hauling, mining) with their own needs (power, maintenance), and
**factories** that run bills without crafters once supplied with input and
power/mana. Designed so robots are just pawns with a different "brain" profile
and need set — the job system stays shared.

## 8. Milestone 1 — Core Loop Slice

Status as of 2026-11-08:

- [x] Workspace, ECS with tests
- [x] Defs (RON) + hot reload (defs only; sprites need a restart)
- [x] Deterministic sim: clock, RNG, map gen with z-levels, pathfinding
- [x] Pawns: races (Human, Elf, Dwarf), needs, mood/thoughts, skills, quirks, stats
- [x] Jobs: mine, dig stairs, chop, harvest, haul, deliver/build, craft, eat, sleep, relax; mental breaks
- [x] Stockpiles (category filters in data, no filter UI yet), blueprints, workbench bills
- [x] Save/load + determinism tests (save → load → continue is hash-identical)
- [x] Blender bake pipeline (procedural low-poly models, pixel post-process, atlas + manifest)
- [x] wgpu renderer: atlas, iso projection, 4 camera angles, cutaway, integer zoom, day/night tint
- [x] Bespoke pixel UI: top bar, colonist list, toolbar, build menu, inspector (pawn/bench/bills), work grid, log, debug overlay
- [x] Input & tools: designate (drag), zone, build (drag walls, R rotate), select

Deliberately simplified / deferred from the M1 plan:

- No constructed floors and no "build stairs up" — vertical travel is natural ramps + dug stairs.
- No crafting quality yet (the `CraftQuality` stat is not wired).
- All buildings are 1×1; benches and beds are passable.
- Stockpile filter toggles exist in the sim but have no UI (added in M2, §10.1).
- Normal-map lighting not started (global light tint only).

Debug tooling: `simrun` (headless runner with colony reports and ASCII maps),
`colony --shot` (offscreen screenshots with scripted orders/camera), the F3
overlay, and `sim::scenario` (scripted player orders used by tests and tools).

## 9. Milestone 1.5 — Art Upgrade

Decided 2026-11-08. Done *before* M2 so every new asset is made once, at final
size and with lighting. **Status: done** (all items below implemented).

- **Scale:** cells become **48 × 24 px** diamonds and one z-level **30 px**
  (true 2:1 cube height). Colonists ~36 px tall, enough for faces, hair, gear and
  held tools. `iso.rs` constants and the bake camera change together.
- **Palette:** keep the current muted, naturalistic palette.
- **Outlines:** *selective*: each outline pixel is a darkened version of the
  adjacent sprite colour instead of near-black. Terrain blocks stay unoutlined.
- **Terrain variety:** several baked variants per block, ramp and plant (cracks,
  pebbles, tufts), chosen per cell by a stable hash of the coordinates.
- **Real lighting:** the bake also emits a normal map (and height/depth) per
  frame. The renderer lights sprites from the sun (day/night, per camera angle)
  and from point lights (campfires, glowing devices, later magic). Underground is
  dark unless lit. Light sources are a def property of buildings/items.
- **Colonist variety:** per-colonist appearance genes (skin, hair colour/style,
  clothing colours, build) from race-specific palettes; held tools shown during
  work (pickaxe, axe, hammer, ...). Implemented as layered sprites (body, hair,
  clothes, tool) baked separately and recoloured via palette ramps, so the bake
  does not grow combinatorially.
- **Juice & feedback:** work particles (chips, sparks, leaves), floating text
  (level-ups, mood events), smooth cutaway/level transitions.
- **Expression:** small icon **speech/thought bubbles** over colonists
  (chatting, hungry, tired, angry, love, breakdown). No name tags in the world.

## 10. Milestone 2 — Colony Depth

Decided 2026-11-08. Focus: make the core loop richer before adding big new
systems (power/mana, automation, combat come after).

### 10.1 Farming — growing zones
Decided 2026-11-08 (interview). **Status: implemented** (`crates/sim/src/farm.rs`,
crop/seed defs in `assets/defs/plants` and `items`, tests in `crates/sim/tests/farm.rs`).
Paint a growing zone, pick a crop; colonists sow, harvest and clear it
automatically (Plants work type, Botany skill).

- **Seeds are items.** Each crop names a seed item; sowing a cell consumes one.
  Harvests return some seeds (they are just part of the harvest `yields`), and
  wild duskberry bushes drop a few berry seeds when harvested. Colonists arrive
  with a few seed bags. New `Seed` item category (stockpiles accept it), never
  eaten. A sow job picks up a handful of seeds and sows several nearby cells in
  one trip; leftovers are dropped and hauled back.
- **Growth is data-driven through the env fields.** A plant def lists `growth`
  factors: piecewise-linear curves over any env field (temperature, light, ...),
  multiplied together with soil fertility. So ordinary crops stop growing in the
  dark and in the cold, and cave fungus grows *only* in the dark. Seasons act
  through the climate temperature curve. `grow_days` is time at a 100 % rate;
  light-hungry crops rest at night, so real time is roughly double.
- **Plant health.** Plants have health 0..1. `harm` entries (field below / above
  a threshold, health lost per day) drain it, for example frost or heat. It
  recovers slowly (`heal_per_day`) when nothing harms it. The plant dies at 0,
  and harvest yield scales with health. One cold night hurts a field but does not
  wipe it out; a winter kills unprotected crops.
- **Fertility.** Terrain defs get a `fertility` multiplier (soil 1.0, grassy soil
  1.1, rock 0). A crop's `min_fertility` gates where it can be sown, and
  `fertility_weight` says how much fertility matters (0 for fungus, which grows
  on bare rock floors). Fertility comes from the solid block under the cell.
- **Annual vs perennial.** `HarvestDef.remove` removes the plant on harvest
  (annual crops). Otherwise it regrows to `regrow_to` (bushes).
- **Zones.** Growing zones share the zone id space with stockpiles (a cell
  holds one zone). Each has a crop and a *sow* toggle. In a zone, colonists:
  harvest ripe crops of any kind (no designation needed), cut down foreign
  plants (wild scrub, a crop the zone no longer wants), and sow empty cells
  that are fertile enough and free of items and buildings.
- **UI.** A *Grow zone* tool drags a zone like a stockpile and defaults to the
  first crop. Clicking an empty zone cell selects the zone: the inspector shows
  a crop picker (with seed counts), the sow toggle and status. The plant
  inspector shows growth, health, the current growth rate and what limits it.
- **First crop set.** *Ashroot* (fast, hardy tuber, modest yield), *Rustwheat*
  (slow grain, big yield, cold-sensitive), *Gloomcap* (cave fungus that needs
  darkness and grows on rock, a niche for dwarves underground), *Threadflax*
  (fiber for later cloth). Duskberry becomes sowable too. Campfire recipes
  cook each food into simple meals.

Work order within the Plants work type: the nearest of designated chop/harvest,
zone harvest and zone clearing first; sowing only when none of those is left.
Colonists won't sow a cell where the crop would currently be harmed (no
planting into frost).

Implementation notes and first balance pass:

- Plants update every 250 ticks (`farm::sys_plants`); deaths are logged once
  per plant type and cause ("6 Rustwheat plants died of frost.").
- Harvested produce and seeds drop on the cell; a cell with an item on it isn't
  sown until it's hauled away, so farms want a stockpile nearby.
- A sow job carries up to 6 seeds and sows cells of the same zone within 8
  cells of the first; a cell that stops needing seed mid-job is skipped.
- Wild ashpines, scrub and duskberries get temperature growth curves (dormant
  in winter) but no harm, so the wild ecology survives the year.
- Measured with `simrun`: an ashroot field sown on day 1 (cold early spring) is
  ripe around day 9; in summer a cycle takes about 7 days. 24 cells of ashroot
  feed roughly half of a 3-colonist colony.
- `simrun` reports each growing zone (planted, growth, health, current rate);
  `colony --shot ... --zone` selects the first growing zone.
- Stockpile zones also got an inspector with category filter toggles,
  including the new `Seed` category.

### 10.2 New races
Orcs, Mutants (Wastefolk), Lupine, Ursine, Feline (see §2.3). Each brings its own
needs, aptitudes, quirk weights, body plan and sprite parameters. Synthetics wait
until power and maintenance systems exist.

### 10.3 Social — opinions & relationships
Decided 2026-11-08 (interview).

- **Opinions.** Pairwise opinion −100..100, the sum of decaying *opinion
  memories* ("Chatted", "Insulted me", "Ate together") and standing terms
  (relationship, race attitude, quirks).
- **Interactions.** Opportunistic: nearby idle or working colonists roll for an
  interaction now and then (chat, deep talk, joke, compliment, insult, flirt).
  The odds and outcome depend on Social skill, quirks and current opinion.
  Interactions are data-driven defs, shown as a speech-bubble icon, and give
  Social XP.
- **Social activities.** Colonists also seek each other out:
  - *Shared meals:* new Table and Chair. Eating at a table gives a thought, and
    eating near others gives a chance to chat. New *dining room* room role.
  - *Social recreation:* during recreation, colonists seek out a friend to chat
    with, or join a group at a campfire or table.
  - *Recreation buildings:* two-player game tables (e.g. dice, chess). Playing
    builds opinion and trains Social.
  - *Partners:* partners seek each other out, share a bed and get a strong
    mood boost (kept non-explicit).
- **Relationships.** Friend and rival come from opinion thresholds, with
  hysteresis. Romance: flirting can make two colonists lovers, and lovers can
  become partners. Partners can break up, and jealousy is possible. Data-driven
  race compatibility and quirks gate romance. **Family:** kin (parent, child,
  sibling) are generated with the starting colony and give strong standing
  opinion and bonds. No births yet.
- **Race attitudes.** Data-driven opinion offsets between races that fade as two
  colonists get to know each other (shared interactions). Quirks can amplify the
  offset (e.g. Bigot) or remove it (e.g. Open-minded).
- **Social fights.** An insult between colonists who dislike each other can
  escalate into a short fistfight. It deals blunt damage through the health
  model (§10.4). Fights are non-lethal by default: they stop once a colonist is
  downed or in heavy pain, and blunt hits never destroy vital organs. A
  `lethal` data flag removes these safeguards.
- **Thoughts.** Social thoughts feed mood ("Chatted with Ada", "Insulted by
  Brokk", "My rival is nearby", "Shared a bed with my partner"). Death and grief
  are covered in §10.4.

Implementation notes (`crates/sim/src/social.rs`, `defs/interactions`,
tests in `crates/sim/tests/social.rs`). **Status: implemented.**

- Opinion = Σ decaying opinion memories (same-label memories merge, capped
  at 5× one event) + relationship standing (partner +30, lover +20,
  parent/child +25, sibling +15) + race attitude × (1 − familiarity) × quirk
  `race_bias`. Familiarity grows with every interaction and game.
- Every colonist rolls an interaction roughly every 1–3 hours when someone is
  within 5 cells. That is 3× more often while socializing (hanging out, at a
  game table, eating at a table), and scaled by quirk `chattiness`.
  Interactions: chat, joke, deep talk, compliment, slight, insult, flirt.
  Speech bubbles show each interaction's `icon`.
- Friend at mutual opinion ≥ 30 (kept until < 15). Rival at ≤ −30 (kept until
  > −15). A successful flirt makes lovers, using compatibility (race
  `romance`), the recipient's opinion and the initiator's social impact.
  Lovers with mutual opinion ≥ 50 for 3+ days may commit as partners.
  Someone already taken only flirts with a colonist they like 30+ more than
  their partner, and flirts aimed at a taken colonist succeed 20× less often.
  Partners move into a double bed if either owns one. A soured romance may
  break up. Winning someone who is taken leaves their old partner jilted
  ("Cheated on me" −40).
- Family: the starting colony gets a fourth colonist, a sibling, parent or
  child of one of the others. They share the race and family name, and their
  ages are adjusted to fit. (The three founders are different races, so they
  can't be related.)
- Insults between colonists who dislike each other (opinion ≤ −10, or rivals)
  have a 12 % chance of a brawl (×2 at low mood, quirk `fight_chance`). Brawls
  deal 2–6 blunt damage every 1.5 s; they end when someone is downed, pain
  > 45 % or after 30 s. An interaction def's `lethal_fights` flag lifts the
  non-lethal safeguards.
- Idle colonists of every race sometimes seek company (only humans have a
  Recreation need). Options: join or start a game at a game table (a lone
  player gives up after a while), go to a friend, partner or relative, or
  hang out at a table or campfire.
- At bedtime, partners who share a bed may have *lovin'* (+10 mood, at most
  every 12 h).

### 10.4 Health — full body model (RimWorld level)
Decided 2026-11-08 (interview).

- **Body defs** per race at RimWorld depth (about 30–50 parts): a tree of parts
  (head › skull › brain, eyes, ears, nose, jaw; torso › heart, lungs, liver,
  kidneys, stomach, ribs, spine, neck; arms › shoulders › hands › fingers;
  legs › feet › toes; plus tails, wings or horns for races that have them).
  Each part has HP, coverage (hit chance within its parent), depth (inside or
  outside), tags, and the **capacities** it contributes to.
- **Capacities:** consciousness, moving, manipulation, sight, hearing,
  breathing, blood pumping, blood filtration, metabolism, talking. They are
  calculated from part efficiency and conditions, and feed stats (moving →
  MoveSpeed, manipulation → work speeds, consciousness → everything). A colonist
  is *downed* when consciousness or moving drops too low. Death comes when a
  vital capacity reaches zero, from blood loss, or when the brain or heart is
  destroyed.
- **Hediffs** (health conditions) attach to a part or to the whole body, all
  data-driven: injuries (cut, crush, bruise, burn, bite, frostbite...),
  bleeding, infection, illness, scars, missing parts, pain. Each has severity,
  stages, effects on stats and capacities, tendability, an immunity race
  (illnesses), and lethality. Pain → pain shock (downed).
- **Injury and illness sources in M2:**
  - Social fights (blunt).
  - Work accidents: a small chance of cuts, crushes or burns while mining,
    building or crafting, scaled by skill and quirks (Clumsy). Rare mining
    cave-ins.
  - Illness and food poisoning: data-driven diseases (flu, gut worms, wound
    infection) that race against immunity. Food poisoning from raw or poorly
    cooked meals. Filth (the cleanliness field) raises the odds.
  - Environment exposure: hypothermia, heatstroke and frostbite from the
    temperature field; aether sickness from aether storms.
- **Medicine.** A *Doctor* work type using the Medicine skill. Doctors tend
  wounds and illnesses (stopping bleeding, slowing infection, improving the
  immunity race). Treatment quality comes from skill and the medicine used:
  none, herbal medicine (from a new crop), or an industrial med kit (crafted).
  Medical beds boost recovery. Injured or sick colonists rest in bed
  automatically. *Rescue* carries downed colonists to a bed.
  **Extensibility:** treatments are data-driven *treatment sources*, so
  magic-themed healing (healing spells, regeneration salves, mana-infused beds,
  part regrowth) slots in later without code changes. Surgery and prosthetics
  come later.
- **Death.** A dead colonist leaves a *corpse* item that rots over time. Rotting
  raises filth, and seeing a corpse gives a bad thought. A *graveyard* zone with
  graves: colonists haul and bury corpses. Grief thoughts for friends, partners
  and family scale with how close they were.
- **UI.** A Health tab in the colonist inspector: a capacities list, conditions
  grouped by body part (severity, bleeding, tended state), total pain, and a
  **pixel-art body diagram** per body plan that colours each part by damage.
  Injured colonists get an icon over their sprite. Bleeding drops blood filth.
- Built so M5 combat reuses the same damage path (damage type → hit part →
  injury).

Implementation notes (`health.rs`, `medical.rs`, `hazards.rs`, `death.rs`,
defs `bodies`, `hediffs`, `capacities`, `damage_types`; tests in
`crates/sim/tests/health.rs`). **Status: implemented.**

- Humanoid body: 50 parts (5 fingers and 5 toes per side). A capacity is the
  weighted average of its contributing parts' efficiency (HP fraction; 0 if
  the part or an ancestor is missing), × `critical_for` parts (spine →
  moving, neck → breathing), + condition offsets + pain × `pain`, ×
  `depends` on earlier capacities. Capacities feed stats through
  lerp(1, value, weight).
- Downed below 30 % consciousness or 15 % moving, or in pain shock (> 80 %).
  Death: a `kills` part destroyed (brain, heart), a vital capacity at 0, or a
  condition reaching `lethal_at` (blood loss, infection, flu, hypothermia...).
- Injury severity is HP lost. Bleeding = Σ bleed × severity of untended
  wounds (missing parts bleed for 2 days), which feeds blood loss. Healing per
  day is lerp(1, tended_heal, quality) × medicine heal_mult × stat
  `injury_heal` × bed rest (1.5 × bed `heal_factor`). Each wound rolls once
  for infection at 12 h. Tending cuts the chance by up to 85 %, and filth
  raises it. A healed wound may scar (permanent HP loss, slight pain).
- Illness: immunity rises (stat `immunity_gain`, faster in bed) while
  severity rises, slowed by tend quality (`tended_factor`). Once immune,
  severity falls. Tends on illnesses last 24 h.
- Exposure is plain data: a hediff `trigger` (condition + field curve) drives
  hypothermia, heatstroke and aether sickness. Hypothermia's serious stages
  spawn frostbite on `extremity` parts.
- Doctor work type (first in the list): rescue downed colonists to a bed
  (medical beds first), tend patients who are downed or in bed (one session
  tends every untended condition, using the best medicine available), and
  feed downed patients. A bleeding doctor with nobody else around tends
  themselves at 70 % quality. Tend quality = stat `tend_quality` (Medicine
  skill) × medicine potency × random 0.85–1.15, capped by the medicine's
  `max_quality`. No medicine: potency 0.5, cap 0.7. Herbal medicine (the
  *Mendleaf* crop): 0.8 / 0.85. Med kit (workbench: 2 herbal + 4 fiber +
  1 metal parts): 1.15 / 1.0, heals 15 % faster.
- Sources: accidents are rolled every 250 work ticks: mining 0.4 %,
  building 0.3 %, crafting 0.3 % (burns at fire benches), chopping 0.2 %.
  They are scaled by skill and stat `accident_chance` (Clumsy ×2). Cave-ins:
  0.06 % per roll when mining under rock, 2–4 crush hits. Random illness:
  2 %/colonist/day, + filth. Random hits from accidents and fists land on
  outer parts only; cave-ins can reach organs.
- Any untended wound or illness sends the colonist to bed (or, with no free
  bed, to lie down where they are) until a doctor has tended it. Food poisoning: item `poison_chance` (raw food
  1.2 %, meals 0.4 %) × filth × stat `food_poison_chance` (Iron Stomach 0).
- Death leaves a *corpse* item (beauty −4; never stockpiled). After 2 days it
  rots and adds filth. "Saw a corpse" is a situational thought. Graves are a
  buildable `Grave` building (dug, no materials), not a zone. Hauling buries
  corpses in empty graves, and filled graves get their own sprite. Grief
  thoughts depend on the relationship: partner −25, lover −20, kin −18,
  friend −10, rival +4, anyone else −3.
- Client: the colonist inspector has Overview, Health and Social tabs. Health
  shows status, pain, bleeding, the body diagram (hover a part for its HP and
  conditions), capacities and conditions by part. Social lists each colonist
  with relation and opinion; hover shows the opinion breakdown. Colonists who
  are downed, resting or carried are drawn lying down (carried ones on the
  carrier's shoulders), and a red + marks the bleeding or downed.

### 10.5 Environment — generic field system
**Status: implemented** (`crates/sim/src/env.rs`, `assets/defs/env_fields`,
`room_roles`, `weather`). Notes on the implementation:

- Fields are `Dynamic` (stateful grid: emitters → diffusion → relax toward
  ambient; walls block, doors leak 25 %, solid rock is a boundary at the
  climate mean) or `Static` (recomputed from `inputs` like `Field(filth)` /
  `Daylight` plus emitters). Updated every 60 ticks.
- Emitters: `env` lists on building/item/plant defs; building `light` defs feed
  the `light` field automatically.
- Changes between updates (foot traffic, cleaning) are queued and applied at
  the next update, so room aggregates are a pure function of saved state
  (save → load stays bit-identical).
- Conditions: `FieldBelow/Above`, `RoomFieldBelow/Above`, `TooLow/TooHigh`
  (race `comfort` ranges), `Indoors`, `InRoomRole`, `Impressiveness*`,
  `Weather`. `Underground` now means rock overhead (constructed roofs don't count).
- Rooms: 4-connected regions per level bounded by rock/walls/doors; indoor if
  enclosed, fully roofed and ≤ 1200 cells. Roles from building counts
  (bedroom, barracks, kitchen, workshop). Impressiveness = Σ field weights ×
  room aggregate + size term.
- Seasons: 15-day seasons, 60-day year; climate ambient follows season and
  time of day. Weather: clear, overcast, rain, fog, aether storm.
- Roofs: Roof / No roof designations, built by Construction. Cleaning work
  type removes filth in indoor rooms.
- Client: field/room overlays (O), weather visuals, filth decals, cut-down
  walls on the viewed level (V toggles all), env info in tooltips/inspector.

Original design notes:

- An `EnvFieldDef` (defs/env_fields) declares a scalar field: name, default,
  range, resolution (per cell or per room), **sources** (buildings, items,
  terrain, weather, time of day), propagation (diffusion through open cells,
  blocked by walls/doors, leaking to the outdoors), decay toward an ambient
  value, and how room aggregates are computed (average, sum, min, max).
- Initial fields: **temperature** (seasonal/daily outdoor curve, heaters and
  coolers, diffusion between rooms, insulation by walls/roofs), **beauty**
  (from buildings/items/plants and from filth), **cleanliness** (filth items or
  per-cell dirt from traffic, blood, debris; a Cleaning work type), **light**
  (shared with the renderer's light sources).
- Conditions and stat modifiers can query any field
  (`FieldBelow("temperature", 5)`, `RoomFieldAbove("beauty", 3)`), so race
  comfort ranges, quirks and thoughts react to new fields with no code changes.
  A new factor (noise, smell, aether radiation, wetness...) is mostly a new
  def file.
- **Rooms:** auto-detected enclosed regions (walls, doors, natural rock, roof)
  with a cached region graph. Each room gets a **role** from its contents
  (bedroom, barracks, kitchen, workshop, dining room, hospital, storeroom) and an
  **impressiveness** score from size, beauty, cleanliness and space. Roles and
  scores are defs-driven and produce thoughts.
- **Weather:** a weather def system (clear, rain, fog, heatwave, cold snap,
  **aether storm**) that sets outdoor field values, light, movement and work
  modifiers and thoughts. Aether storms are setting flavour now and drive
  mutations / magic events later.
- **Roofs:** needed for "indoors". Constructed roofs (roof designation areas)
  are added alongside constructed floors.

## 11. Roadmap (post-M2)

- **M3 — Power & Mana:** electric and mana networks, generators, research, first
  magic/tech hybrid buildings.
- **M4 — Automation:** Synthetics, robots, factories, conveyors-lite.
- **M5 — Danger:** combat, raids, wildlife, random event storyteller.
- **M6 — Progression:** tech/arcana trees, overworld, expeditions.
