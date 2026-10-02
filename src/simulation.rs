use std::collections::HashSet;

use crate::chunk::Chunk;
use crate::coords::{MAX_CHUNK_COORD, MIN_CHUNK_COORD};
use crate::world::World;
use crate::{CHUNK_SIDE, CellCoord, ChunkCoord};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GenerationDelta {
    pub changed_chunks: Vec<ChunkCoord>,
    pub born_chunks: Vec<ChunkCoord>,
    pub removed_chunks: Vec<ChunkCoord>,
}

/// Computes one generation without modifying `current`.
pub fn next_generation(current: &World) -> (World, GenerationDelta) {
    let candidates = candidate_chunks(current);
    let mut next = World::new();

    for (chunk_x, chunk_y) in candidates {
        let origin_x = chunk_x * CHUNK_SIDE as i64;
        let origin_y = chunk_y * CHUNK_SIDE as i64;
        let mut next_chunk = Chunk::default();

        for local_y in 0..CHUNK_SIDE {
            for local_x in 0..CHUNK_SIDE {
                let cell = CellCoord {
                    x: origin_x + local_x as i64,
                    y: origin_y + local_y as i64,
                };
                let alive = current.get(cell);
                let neighbors = count_live_neighbors(current, cell);
                if neighbors == 3 || (alive && neighbors == 2) {
                    next_chunk.set(local_x as u8, local_y as u8, true);
                }
            }
        }

        next.insert_chunk((chunk_x, chunk_y), next_chunk);
    }

    let delta = generation_delta(current, &next);
    (next, delta)
}

fn candidate_chunks(world: &World) -> HashSet<ChunkCoord> {
    let mut candidates = HashSet::new();

    for ((chunk_x, chunk_y), _) in world.chunks() {
        for dy in -1_i64..=1 {
            for dx in -1_i64..=1 {
                let (Some(x), Some(y)) = (chunk_x.checked_add(dx), chunk_y.checked_add(dy)) else {
                    continue;
                };
                if (MIN_CHUNK_COORD..=MAX_CHUNK_COORD).contains(&x)
                    && (MIN_CHUNK_COORD..=MAX_CHUNK_COORD).contains(&y)
                {
                    candidates.insert((x, y));
                }
            }
        }
    }

    candidates
}

fn count_live_neighbors(world: &World, cell: CellCoord) -> u8 {
    let mut count = 0;

    for dy in -1_i64..=1 {
        for dx in -1_i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }

            let (Some(x), Some(y)) = (cell.x.checked_add(dx), cell.y.checked_add(dy)) else {
                continue;
            };
            if world.get(CellCoord { x, y }) {
                count += 1;
            }
        }
    }

    count
}

fn generation_delta(current: &World, next: &World) -> GenerationDelta {
    let mut coordinates: HashSet<_> = current.chunks().map(|(coord, _)| coord).collect();
    coordinates.extend(next.chunks().map(|(coord, _)| coord));

    let mut delta = GenerationDelta::default();
    for coord in coordinates {
        let previous = current.chunk(coord);
        let following = next.chunk(coord);
        if previous == following {
            continue;
        }

        delta.changed_chunks.push(coord);
        match (previous, following) {
            (None, Some(_)) => delta.born_chunks.push(coord),
            (Some(_), None) => delta.removed_chunks.push(coord),
            _ => {}
        }
    }

    delta.changed_chunks.sort_unstable();
    delta.born_chunks.sort_unstable();
    delta.removed_chunks.sort_unstable();
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(cells: &[(i64, i64)]) -> World {
        let mut world = World::new();
        for &(x, y) in cells {
            world.set(CellCoord { x, y }, true);
        }
        world
    }

    #[test]
    fn empty_world_stays_empty() {
        let world = World::new();

        let (next, delta) = next_generation(&world);

        assert!(next.is_empty());
        assert_eq!(delta, GenerationDelta::default());
    }

    #[test]
    fn block_is_a_still_life_across_a_chunk_corner() {
        let block = world_with(&[(-1, 63), (0, 63), (-1, 64), (0, 64)]);

        let (next, delta) = next_generation(&block);

        assert_eq!(next, block);
        assert_eq!(delta, GenerationDelta::default());
    }

    #[test]
    fn blinker_oscillates_across_chunk_edges_and_reports_delta() {
        let horizontal = world_with(&[(62, 63), (63, 63), (64, 63)]);
        let vertical = world_with(&[(63, 62), (63, 63), (63, 64)]);

        let (next, delta) = next_generation(&horizontal);
        assert_eq!(next, vertical);
        assert_eq!(delta.changed_chunks, vec![(0, 0), (0, 1), (1, 0)]);
        assert_eq!(delta.born_chunks, vec![(0, 1)]);
        assert_eq!(delta.removed_chunks, vec![(1, 0)]);

        let (following, _) = next_generation(&next);
        assert_eq!(following, horizontal);
    }

    #[test]
    fn beacon_returns_to_its_original_state_after_two_generations() {
        let beacon = world_with(&[
            (62, 62),
            (63, 62),
            (62, 63),
            (63, 63),
            (64, 64),
            (65, 64),
            (64, 65),
            (65, 65),
        ]);

        let (after_one, _) = next_generation(&beacon);
        let (after_two, _) = next_generation(&after_one);

        assert_ne!(after_one, beacon);
        assert_eq!(after_two, beacon);
    }

    #[test]
    fn glider_moves_across_chunk_edges_and_a_corner() {
        let glider = world_with(&[(63, 62), (64, 63), (62, 64), (63, 64), (64, 64)]);
        let moved = world_with(&[(64, 63), (65, 64), (63, 65), (64, 65), (65, 65)]);

        let mut generation = glider;
        for _ in 0..4 {
            generation = next_generation(&generation).0;
        }

        assert_eq!(generation, moved);
    }

    #[test]
    fn lone_cell_dies_and_its_chunk_is_removed() {
        let lone_cell = world_with(&[(10, -10)]);

        let (next, delta) = next_generation(&lone_cell);

        assert!(next.is_empty());
        assert_eq!(delta.changed_chunks, vec![(0, -1)]);
        assert!(delta.born_chunks.is_empty());
        assert_eq!(delta.removed_chunks, vec![(0, -1)]);
    }

    #[test]
    fn coordinate_limits_do_not_overflow_neighbor_calculation() {
        let edge = world_with(&[(i64::MAX, i64::MAX)]);

        let (next, delta) = next_generation(&edge);

        assert!(next.is_empty());
        assert_eq!(
            delta.removed_chunks,
            vec![(MAX_CHUNK_COORD, MAX_CHUNK_COORD)]
        );
    }
}
