//! A* pathfinding and Dijkstra flood-fill searches over the cell grid.
//!
//! Buffers are reused between searches via generation stamps, so a search
//! costs nothing proportional to map size up front.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::geom::P3;
use crate::map::Map;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Goal {
    /// Stand exactly on the cell.
    On(P3),
    /// Stand on a cell 8-adjacent (same level) to the target, not on it.
    Adjacent(P3),
    /// Either on or adjacent.
    Near(P3),
}

impl Goal {
    pub fn target(self) -> P3 {
        match self {
            Goal::On(p) | Goal::Adjacent(p) | Goal::Near(p) => p,
        }
    }

    pub fn satisfied_by(self, p: P3) -> bool {
        match self {
            Goal::On(t) => p == t,
            Goal::Adjacent(t) => p != t && p.is_adjacent8(t),
            Goal::Near(t) => p.is_adjacent8(t),
        }
    }
}

#[derive(Default)]
pub struct Pathfinder {
    stamp: Vec<u32>,
    closed: Vec<u32>,
    cost: Vec<u32>,
    parent: Vec<u32>,
    cur_gen: u32,
    heap: BinaryHeap<Reverse<(u32, u32, u32)>>,
    /// Statistics for debugging/profiling.
    pub searches: u64,
    pub nodes_expanded: u64,
}

const NONE: u32 = u32::MAX;

fn heuristic(a: P3, b: P3) -> u32 {
    let dx = (a.x - b.x).unsigned_abs();
    let dy = (a.y - b.y).unsigned_abs();
    let dz = (a.z - b.z).unsigned_abs();
    let oct = 10 * dx.max(dy) + 4 * dx.min(dy);
    oct.max(14 * dz)
}

impl Pathfinder {
    fn begin(&mut self, n: usize) {
        if self.stamp.len() != n {
            self.stamp = vec![0; n];
            self.closed = vec![0; n];
            self.cost = vec![0; n];
            self.parent = vec![NONE; n];
            self.cur_gen = 0;
        }
        self.cur_gen = self.cur_gen.wrapping_add(1);
        if self.cur_gen == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.closed.iter_mut().for_each(|s| *s = 0);
            self.cur_gen = 1;
        }
        self.heap.clear();
        self.searches += 1;
    }

    fn reconstruct(&self, map: &Map, end: usize) -> Vec<P3> {
        let mut out = Vec::new();
        let mut i = end as u32;
        while i != NONE {
            out.push(map.pos_of(i as usize));
            i = self.parent[i as usize];
        }
        out.reverse();
        out
    }

    /// A* from `start` to `goal`. Returned path includes `start` as first cell.
    pub fn find_path(&mut self, map: &Map, start: P3, goal: Goal, max_nodes: usize) -> Option<Vec<P3>> {
        if !map.in_bounds(start) {
            return None;
        }
        if goal.satisfied_by(start) {
            return Some(vec![start]);
        }
        self.begin(map.len());
        let target = goal.target();
        let si = map.idx(start);
        self.stamp[si] = self.cur_gen;
        self.cost[si] = 0;
        self.parent[si] = NONE;
        let mut counter = 0u32;
        self.heap.push(Reverse((heuristic(start, target), counter, si as u32)));
        let mut expanded = 0usize;
        while let Some(Reverse((_, _, ci))) = self.heap.pop() {
            let ci = ci as usize;
            if self.closed[ci] == self.cur_gen {
                continue;
            }
            self.closed[ci] = self.cur_gen;
            let cp = map.pos_of(ci);
            if goal.satisfied_by(cp) {
                return Some(self.reconstruct(map, ci));
            }
            expanded += 1;
            self.nodes_expanded += 1;
            if expanded > max_nodes {
                return None;
            }
            let base = self.cost[ci];
            let g = self.cur_gen;
            let mut pushes: [(u32, u32); 16] = [(0, 0); 16];
            let mut np = 0;
            map.for_each_neighbor(cp, |q, c| {
                if np < 16 {
                    pushes[np] = (map.idx(q) as u32, c);
                    np += 1;
                }
            });
            for &(ni, c) in &pushes[..np] {
                let ni_u = ni as usize;
                if self.closed[ni_u] == g {
                    continue;
                }
                let nc = base + c;
                if self.stamp[ni_u] != g || nc < self.cost[ni_u] {
                    self.stamp[ni_u] = g;
                    self.cost[ni_u] = nc;
                    self.parent[ni_u] = ci as u32;
                    counter += 1;
                    let h = heuristic(map.pos_of(ni_u), target);
                    self.heap.push(Reverse((nc + h, counter, ni)));
                }
            }
        }
        None
    }

    /// Dijkstra flood fill from `start`, visiting walkable cells in order of
    /// path cost. `visit` returns `Some` to stop; the result includes the path
    /// to the cell where the search stopped.
    pub fn search<T>(
        &mut self,
        map: &Map,
        start: P3,
        max_cost: u32,
        mut visit: impl FnMut(P3) -> Option<T>,
    ) -> Option<(T, Vec<P3>)> {
        if !map.in_bounds(start) {
            return None;
        }
        self.begin(map.len());
        let si = map.idx(start);
        self.stamp[si] = self.cur_gen;
        self.cost[si] = 0;
        self.parent[si] = NONE;
        let mut counter = 0u32;
        self.heap.push(Reverse((0, 0, si as u32)));
        while let Some(Reverse((c, _, ci))) = self.heap.pop() {
            let ci = ci as usize;
            if self.closed[ci] == self.cur_gen {
                continue;
            }
            self.closed[ci] = self.cur_gen;
            let cp = map.pos_of(ci);
            self.nodes_expanded += 1;
            if let Some(t) = visit(cp) {
                return Some((t, self.reconstruct(map, ci)));
            }
            let g = self.cur_gen;
            let mut pushes: [(u32, u32); 16] = [(0, 0); 16];
            let mut np = 0;
            map.for_each_neighbor(cp, |q, c| {
                if np < 16 {
                    pushes[np] = (map.idx(q) as u32, c);
                    np += 1;
                }
            });
            for &(ni, sc) in &pushes[..np] {
                let ni_u = ni as usize;
                if self.closed[ni_u] == g {
                    continue;
                }
                let nc = c + sc;
                if nc > max_cost {
                    continue;
                }
                if self.stamp[ni_u] != g || nc < self.cost[ni_u] {
                    self.stamp[ni_u] = g;
                    self.cost[ni_u] = nc;
                    self.parent[ni_u] = ci as u32;
                    counter += 1;
                    self.heap.push(Reverse((nc, counter, ni)));
                }
            }
        }
        None
    }

    /// Collects every cell reachable from `start` within `max_cost`.
    pub fn reachable(&mut self, map: &Map, start: P3, max_cost: u32) -> Vec<P3> {
        let mut out = Vec::new();
        self.search::<()>(map, start, max_cost, |p| {
            out.push(p);
            None
        });
        out
    }
}
