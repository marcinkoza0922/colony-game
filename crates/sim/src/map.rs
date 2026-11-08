use ecs::Entity;
use serde::{Deserialize, Serialize};

use crate::defs::{BuildingKind, Defs, TerrainId};
use crate::geom::{Dir4, NEIGHBORS8, P3};

/// Player designation on a cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Designation {
    #[default]
    None,
    /// Mine the solid cell.
    Mine,
    /// Dig a staircase into the solid cell below this (open) cell.
    DigStairs,
    /// Chop the tree on this cell.
    Chop,
    /// Harvest the plant on this cell.
    Harvest,
    /// Build a roof over this cell.
    BuildRoof,
    /// Remove the roof over this cell.
    RemoveRoof,
}

/// One map cell. A cell is either solid terrain or open space; open cells can
/// hold one building (or blueprint), one plant and one item stack.
#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub solid: Option<TerrainId>,
    /// Natural slope rising toward the given direction (open cells only).
    pub ramp: Option<Dir4>,
    pub building: Option<Entity>,
    /// Cached kind of `building` (for fast pathing); `None` for blueprints.
    pub building_kind: Option<BuildingKind>,
    pub plant: Option<Entity>,
    pub item: Option<Entity>,
    pub designation: Designation,
    pub zone: Option<u16>,
    /// Constructed roof covering this cell.
    pub roof: bool,
}

#[derive(Clone, Debug)]
pub struct Map {
    pub w: i32,
    pub h: i32,
    pub d: i32,
    cells: Vec<Cell>,
    /// Bumped on any change that affects rendering / pathing (cheap dirty flag).
    pub revision: u64,
    /// Bumped only on structural changes (terrain, walls/doors, roofs) that
    /// can change rooms.
    pub struct_rev: u64,
}

impl Map {
    pub fn new(w: i32, h: i32, d: i32) -> Self {
        Self { w, h, d, cells: vec![Cell::default(); (w * h * d) as usize], revision: 0, struct_rev: 1 }
    }

    #[inline]
    pub fn in_bounds(&self, p: P3) -> bool {
        p.x >= 0 && p.y >= 0 && p.z >= 0 && p.x < self.w && p.y < self.h && p.z < self.d
    }

    #[inline]
    pub fn idx(&self, p: P3) -> usize {
        ((p.z * self.h + p.y) * self.w + p.x) as usize
    }

    pub fn pos_of(&self, idx: usize) -> P3 {
        let i = idx as i32;
        let x = i % self.w;
        let y = (i / self.w) % self.h;
        let z = i / (self.w * self.h);
        P3::new(x, y, z)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    #[inline]
    pub fn get(&self, p: P3) -> Option<&Cell> {
        if self.in_bounds(p) { Some(&self.cells[self.idx(p)]) } else { None }
    }

    #[inline]
    pub fn get_mut(&mut self, p: P3) -> Option<&mut Cell> {
        if self.in_bounds(p) {
            let i = self.idx(p);
            self.revision += 1;
            Some(&mut self.cells[i])
        } else {
            None
        }
    }

    /// Like `get_mut` but panics out of bounds.
    #[track_caller]
    pub fn cell_mut(&mut self, p: P3) -> &mut Cell {
        self.get_mut(p).unwrap_or_else(|| panic!("cell {p} out of bounds"))
    }

    #[track_caller]
    pub fn cell(&self, p: P3) -> &Cell {
        self.get(p).unwrap_or_else(|| panic!("cell {p} out of bounds"))
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    #[inline]
    pub fn is_solid(&self, p: P3) -> bool {
        self.get(p).is_some_and(|c| c.solid.is_some())
    }

    /// Open and not blocked by a wall.
    #[inline]
    pub fn is_passable(&self, p: P3) -> bool {
        match self.get(p) {
            Some(c) => c.solid.is_none() && c.building_kind != Some(BuildingKind::Wall),
            None => false,
        }
    }

    /// Something to stand on below `p`: bedrock (z = 0), solid terrain, or a
    /// staircase coming up from below.
    #[inline]
    pub fn is_supported(&self, p: P3) -> bool {
        if p.z == 0 {
            return true;
        }
        match self.get(p.down()) {
            Some(c) => c.solid.is_some() || c.building_kind == Some(BuildingKind::Stairs),
            None => false,
        }
    }

    #[inline]
    pub fn is_walkable(&self, p: P3) -> bool {
        self.is_passable(p) && self.is_supported(p)
    }

    /// Topmost walkable z in a column at or below `max_z`.
    pub fn surface_z(&self, x: i32, y: i32, max_z: i32) -> Option<i32> {
        (0..=max_z.min(self.d - 1)).rev().find(|&z| self.is_walkable(P3::new(x, y, z)))
    }

    /// True if a constructed roof covers `p` or any solid cell is above it.
    pub fn is_roofed(&self, p: P3) -> bool {
        self.get(p).is_some_and(|c| c.roof) || (p.z + 1..self.d).any(|z| self.is_solid(P3::new(p.x, p.y, z)))
    }

    /// True if only rock (no constructed roof) covers `p`.
    pub fn is_underground(&self, p: P3) -> bool {
        (p.z + 1..self.d).any(|z| self.is_solid(P3::new(p.x, p.y, z)))
    }

    /// Sets (or clears) the building occupying a cell. Use this instead of
    /// writing `building`/`building_kind` directly so room detection notices.
    pub fn set_building(&mut self, p: P3, e: Option<ecs::Entity>, kind: Option<BuildingKind>) {
        let c = self.cell_mut(p);
        let structural = c.building_kind != kind;
        c.building = e;
        c.building_kind = kind;
        if structural {
            self.struct_rev += 1;
        }
    }

    pub fn set_roof(&mut self, p: P3, roof: bool) {
        let c = self.cell_mut(p);
        if c.roof != roof {
            c.roof = roof;
            self.struct_rev += 1;
        }
    }

    /// Open cells that separate rooms: walls, doors and solid terrain.
    pub fn is_room_boundary(&self, p: P3) -> bool {
        match self.get(p) {
            Some(c) => c.solid.is_some() || matches!(c.building_kind, Some(BuildingKind::Wall) | Some(BuildingKind::Door)),
            None => true,
        }
    }

    /// Calls `f` for every cell reachable in one step from `p` with its move cost
    /// (10 = orthogonal, 14 = diagonal / vertical). Stable order.
    pub fn for_each_neighbor(&self, p: P3, mut f: impl FnMut(P3, u32)) {
        for (i, (dx, dy)) in NEIGHBORS8.iter().enumerate() {
            let q = p.offset(*dx, *dy, 0);
            if !self.is_walkable(q) {
                continue;
            }
            if i >= 4 {
                // Diagonal: no corner cutting.
                if !self.is_passable(p.offset(*dx, 0, 0)) || !self.is_passable(p.offset(0, *dy, 0)) {
                    continue;
                }
                f(q, 14);
            } else {
                f(q, 10);
            }
        }
        let Some(c) = self.get(p) else { return };
        // Up a ramp.
        if let Some(dir) = c.ramp {
            let q = p.step(dir).up();
            if self.is_walkable(q) && self.is_passable(p.up()) {
                f(q, 14);
            }
        }
        // Down a ramp: a ramp below-adjacent that rises toward us.
        if p.z > 0 {
            for dir in Dir4::ALL {
                let r = p.step(dir.opposite()).down();
                if let Some(rc) = self.get(r)
                    && rc.ramp == Some(dir)
                    && self.is_walkable(r)
                    && self.is_passable(r.up())
                {
                    f(r, 14);
                }
            }
        }
        // Stairs.
        if c.building_kind == Some(BuildingKind::Stairs) && self.is_walkable(p.up()) {
            f(p.up(), 14);
        }
        if p.z > 0
            && let Some(below) = self.get(p.down())
            && below.building_kind == Some(BuildingKind::Stairs)
            && self.is_walkable(p.down())
        {
            f(p.down(), 14);
        }
    }

    /// Can a standing pawn work on `target` from `stand`?
    pub fn can_reach_from(&self, stand: P3, target: P3) -> bool {
        stand.is_adjacent8(target)
    }

    /// Mining target check: solid cell, standable orthogonal neighbour.
    pub fn is_mineable(&self, p: P3) -> bool {
        self.is_solid(p)
    }

    pub fn set_solid(&mut self, p: P3, t: Option<TerrainId>) {
        let c = self.cell_mut(p);
        c.solid = t;
        if t.is_some() {
            c.ramp = None;
        }
        self.struct_rev += 1;
    }

    /// Removes ramps that no longer make sense (e.g. after mining the slope's
    /// upper cell).
    pub fn fix_ramps_around(&mut self, p: P3) {
        for dz in -1..=1 {
            for (dx, dy) in NEIGHBORS8 {
                let q = p.offset(dx, dy, dz);
                self.fix_ramp(q);
            }
            self.fix_ramp(p.offset(0, 0, dz));
        }
    }

    fn fix_ramp(&mut self, q: P3) {
        let Some(c) = self.get(q) else { return };
        if let Some(dir) = c.ramp
            && !self.is_solid(q.step(dir))
        {
            self.cell_mut(q).ramp = None;
        }
    }

    pub fn building_kind_from(defs: &Defs, def: crate::defs::BuildingId) -> BuildingKind {
        defs.buildings[def].kind
    }
}

// ---------------------------------------------------------------- serialization
// Terrain is stored as a palette + run-length encoded indices to keep saves small.

#[derive(Serialize, Deserialize)]
struct CellSave {
    ramp: Option<Dir4>,
    building: Option<Entity>,
    building_kind: Option<u8>,
    plant: Option<Entity>,
    item: Option<Entity>,
    designation: Designation,
    zone: Option<u16>,
    roof: bool,
}

#[derive(Serialize, Deserialize)]
struct MapSave {
    w: i32,
    h: i32,
    d: i32,
    palette: Vec<TerrainId>,
    /// (palette index + 1, or 0 for open; run length)
    terrain_runs: Vec<(u16, u32)>,
    /// Sparse list of cells with non-default contents.
    extras: Vec<(u32, CellSave)>,
}

fn kind_to_u8(k: BuildingKind) -> u8 {
    match k {
        BuildingKind::Wall => 0,
        BuildingKind::Door => 1,
        BuildingKind::Bed => 2,
        BuildingKind::Workbench => 3,
        BuildingKind::Stairs => 4,
        BuildingKind::Furniture => 5,
    }
}

fn u8_to_kind(v: u8) -> BuildingKind {
    match v {
        0 => BuildingKind::Wall,
        1 => BuildingKind::Door,
        2 => BuildingKind::Bed,
        3 => BuildingKind::Workbench,
        4 => BuildingKind::Stairs,
        _ => BuildingKind::Furniture,
    }
}

impl Serialize for Map {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut palette: Vec<TerrainId> = Vec::new();
        let mut runs: Vec<(u16, u32)> = Vec::new();
        let mut extras = Vec::new();
        for (i, c) in self.cells.iter().enumerate() {
            let v = match c.solid {
                None => 0u16,
                Some(t) => match palette.iter().position(|p| *p == t) {
                    Some(pi) => pi as u16 + 1,
                    None => {
                        palette.push(t);
                        palette.len() as u16
                    }
                },
            };
            match runs.last_mut() {
                Some((lv, n)) if *lv == v => *n += 1,
                _ => runs.push((v, 1)),
            }
            if c.ramp.is_some()
                || c.building.is_some()
                || c.plant.is_some()
                || c.item.is_some()
                || c.designation != Designation::None
                || c.zone.is_some()
                || c.roof
            {
                extras.push((
                    i as u32,
                    CellSave {
                        ramp: c.ramp,
                        building: c.building,
                        building_kind: c.building_kind.map(kind_to_u8),
                        plant: c.plant,
                        item: c.item,
                        designation: c.designation,
                        zone: c.zone,
                        roof: c.roof,
                    },
                ));
            }
        }
        MapSave { w: self.w, h: self.h, d: self.d, palette, terrain_runs: runs, extras }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Map {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let ms = MapSave::deserialize(d)?;
        let mut map = Map::new(ms.w, ms.h, ms.d);
        let mut i = 0usize;
        for (v, n) in ms.terrain_runs {
            for _ in 0..n {
                if i >= map.cells.len() {
                    return Err(serde::de::Error::custom("terrain runs overflow map"));
                }
                map.cells[i].solid = if v == 0 { None } else { Some(ms.palette[v as usize - 1]) };
                i += 1;
            }
        }
        for (idx, cs) in ms.extras {
            let c = map
                .cells
                .get_mut(idx as usize)
                .ok_or_else(|| serde::de::Error::custom("cell index out of range"))?;
            c.ramp = cs.ramp;
            c.building = cs.building;
            c.building_kind = cs.building_kind.map(u8_to_kind);
            c.plant = cs.plant;
            c.item = cs.item;
            c.designation = cs.designation;
            c.zone = cs.zone;
            c.roof = cs.roof;
        }
        Ok(map)
    }
}
