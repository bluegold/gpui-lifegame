use std::collections::HashMap;

use crate::{CellCoord, ChunkCoord};
use crate::{chunk::Chunk, coords::split_cell_coord};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, cell: CellCoord) -> bool {
        let (chunk_coord, local_x, local_y) = split_cell_coord(cell);
        self.chunks
            .get(&chunk_coord)
            .is_some_and(|chunk| chunk.get(local_x, local_y))
    }

    /// Sets a cell and returns whether its state changed.
    pub fn set(&mut self, cell: CellCoord, alive: bool) -> bool {
        let (chunk_coord, local_x, local_y) = split_cell_coord(cell);
        let current = self
            .chunks
            .get(&chunk_coord)
            .is_some_and(|chunk| chunk.get(local_x, local_y));
        if current == alive {
            return false;
        }

        if alive {
            self.chunks
                .entry(chunk_coord)
                .or_default()
                .set(local_x, local_y, true);
        } else {
            let remove_chunk = if let Some(chunk) = self.chunks.get_mut(&chunk_coord) {
                chunk.set(local_x, local_y, false);
                chunk.is_empty()
            } else {
                false
            };

            if remove_chunk {
                self.chunks.remove(&chunk_coord);
            }
        }

        true
    }

    /// Toggles a cell and returns its new state.
    pub fn toggle(&mut self, cell: CellCoord) -> bool {
        let alive = !self.get(cell);
        self.set(cell, alive);
        alive
    }

    pub fn chunk(&self, coord: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&coord)
    }

    pub fn chunks(&self) -> impl Iterator<Item = (ChunkCoord, &Chunk)> {
        self.chunks.iter().map(|(coord, chunk)| (*coord, chunk))
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::{MAX_CHUNK_COORD, MIN_CHUNK_COORD};

    fn cell(x: i64, y: i64) -> CellCoord {
        CellCoord { x, y }
    }

    #[test]
    fn reads_and_writes_cells_across_chunk_boundaries() {
        let mut world = World::new();
        let cells = [
            cell(-65, -65),
            cell(-64, -64),
            cell(-1, -1),
            cell(0, 0),
            cell(63, 63),
            cell(64, 64),
        ];

        for position in cells {
            assert!(world.set(position, true));
            assert!(world.get(position));
        }
        assert_eq!(world.chunk_count(), 4);

        for position in cells {
            assert!(world.set(position, false));
            assert!(!world.get(position));
        }
        assert!(world.is_empty());
    }

    #[test]
    fn does_not_allocate_empty_chunks_and_removes_the_last_live_cell() {
        let mut world = World::new();
        let position = cell(64, -1);

        assert!(!world.set(position, false));
        assert!(world.is_empty());

        assert!(world.set(position, true));
        assert_eq!(world.chunk_count(), 1);
        assert!(!world.set(position, true));

        assert!(world.set(position, false));
        assert!(world.is_empty());
        assert!(world.chunk((1, -1)).is_none());
    }

    #[test]
    fn toggle_returns_the_new_state_and_removes_empty_chunks() {
        let mut world = World::new();
        let position = cell(-1, 64);

        assert!(world.toggle(position));
        assert!(world.get(position));
        assert_eq!(world.chunk_count(), 1);

        assert!(!world.toggle(position));
        assert!(!world.get(position));
        assert!(world.is_empty());
    }

    #[test]
    fn stores_cells_at_both_i64_boundaries() {
        let mut world = World::new();
        let min = cell(i64::MIN, i64::MIN);
        let max = cell(i64::MAX, i64::MAX);

        assert!(world.set(min, true));
        assert!(world.set(max, true));
        assert!(world.get(min));
        assert!(world.get(max));
        assert!(world.chunk((MIN_CHUNK_COORD, MIN_CHUNK_COORD)).is_some());
        assert!(world.chunk((MAX_CHUNK_COORD, MAX_CHUNK_COORD)).is_some());
    }

    #[test]
    fn iterates_only_nonempty_chunks() {
        let mut world = World::new();
        world.set(cell(0, 0), true);
        world.set(cell(-65, 1), true);

        let mut coords: Vec<_> = world
            .chunks()
            .map(|(coord, chunk)| {
                assert!(!chunk.is_empty());
                coord
            })
            .collect();
        coords.sort_unstable();

        assert_eq!(coords, vec![(-2, 0), (0, 0)]);
    }
}
