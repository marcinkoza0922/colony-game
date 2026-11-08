# Colony

An isometric, pixel-art colony sim set in a sci-fi post-apocalyptic fantasy world:
indirect control, autonomous colonists with races, quirks, skills, needs and moods.
Rust, bespoke ECS, wgpu. See [docs/DESIGN.md](docs/DESIGN.md) for the design.

## Running

```sh
cargo run --release -p client            # play (bin: colony)
cargo run --release -p client -- --seed 7
cargo test --workspace --release         # ECS, sim (determinism, save/load) and UI-interaction tests
```

## Controls

| Input | Action |
|---|---|
| WASD / arrows, middle-drag | Pan |
| Mouse wheel, `+`/`-` | Zoom (integer pixel scales) |
| Q / E | Rotate camera (4 angles) |
| PageUp/PageDown, `.`/`,` | Change view level (cutaway) |
| Space, 1–4 | Pause, game speed |
| Left click / drag | Select, or apply the current tool |
| Right click, Esc | Back to select / deselect |
| M, C, H, X | Mine, Chop, Harvest, Cancel tools |
| Z | Stockpile tool |
| B | Build menu (R rotates the blueprint) |
| Tab | Work priorities |
| F5 / F9 | Quick save / load (`saves/quick.sav`) |
| F3 | Debug overlay |

Editing anything under `assets/defs/` while the game runs hot-reloads it.

## Layout

```
crates/ecs      bespoke ECS (sparse sets, resources, commands, snapshots)
crates/sim      deterministic headless simulation + `simrun` debug runner
crates/client   wgpu renderer, pixel UI, input (binary `colony`)
assets/defs     content (.ron): races, quirks, thoughts, items, recipes...
assets/sprites  baked sprite atlas + manifest (generated)
tools/bake      Blender sprite bake pipeline
```

## Art pipeline

Sprites are low-poly models built procedurally in `tools/bake/models.py`, rendered
by headless Blender with an orthographic camera that matches the game's 2:1 iso
grid exactly (48 px tiles). Each frame is rendered as albedo + normal map (+ layer
ids for colonists), posterised, outlined and packed into `atlas.png` /
`atlas_n.png`. The game lights sprites dynamically from the normals.

```sh
tools/bake/bake.sh             # rebake everything (~25 s)
tools/bake/bake.sh --only pawn # just matching jobs (overwrites the atlas with only those!)
tools/bake/bake.sh --debug     # also dump individual frames
```

## Debug tools

```sh
# Headless sim: scripted orders, periodic colony reports, ASCII map, perf
cargo run --release -p sim --bin simrun -- --days 3 --every 12 --map-end 2

# Offscreen screenshot after N ticks with scripted orders and camera setup
cargo run --release -p client -- --shot out.png --orders --ticks 9000 \
    --select 0 --zoom 3 --angle 1 [--viewz 2] [--work] [--build] [--debug] [--mouse X,Y] \
    [--frames 40]   # run live frames first (animation, particles)
```
