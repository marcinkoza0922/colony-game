use serde::{Deserialize, Serialize};

/// Integer cell coordinate on the map. `z` is the level (0 = deepest).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default, Serialize, Deserialize)]
pub struct P3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl P3 {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn up(self) -> Self {
        Self { z: self.z + 1, ..self }
    }

    pub fn down(self) -> Self {
        Self { z: self.z - 1, ..self }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self { x: self.x + dx, y: self.y + dy, z: self.z + dz }
    }

    pub fn step(self, d: Dir4) -> Self {
        let (dx, dy) = d.delta();
        self.offset(dx, dy, 0)
    }

    /// Chebyshev distance on the same level plus level difference.
    pub fn chebyshev(self, o: P3) -> i32 {
        (self.x - o.x).abs().max((self.y - o.y).abs()) + (self.z - o.z).abs()
    }

    pub fn manhattan(self, o: P3) -> i32 {
        (self.x - o.x).abs() + (self.y - o.y).abs() + (self.z - o.z).abs()
    }

    /// Same level and within one step (8-neighbourhood, including self).
    pub fn is_adjacent8(self, o: P3) -> bool {
        self.z == o.z && (self.x - o.x).abs() <= 1 && (self.y - o.y).abs() <= 1
    }

    /// Same level and orthogonally adjacent or identical.
    pub fn is_adjacent4(self, o: P3) -> bool {
        self.z == o.z && (self.x - o.x).abs() + (self.y - o.y).abs() <= 1
    }
}

impl std::fmt::Display for P3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

/// Cardinal direction in map space. North = -y.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum Dir4 {
    N,
    E,
    S,
    W,
}

impl Dir4 {
    pub const ALL: [Dir4; 4] = [Dir4::N, Dir4::E, Dir4::S, Dir4::W];

    pub fn delta(self) -> (i32, i32) {
        match self {
            Dir4::N => (0, -1),
            Dir4::E => (1, 0),
            Dir4::S => (0, 1),
            Dir4::W => (-1, 0),
        }
    }

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Self {
        Self::ALL[(i % 4) as usize]
    }

    pub fn opposite(self) -> Self {
        Self::from_index(self.index() + 2)
    }
}

/// 8-neighbourhood offsets, orthogonals first (stable order for determinism).
pub const NEIGHBORS8: [(i32, i32); 8] = [(0, -1), (1, 0), (0, 1), (-1, 0), (1, -1), (1, 1), (-1, 1), (-1, -1)];
pub const NEIGHBORS4: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
