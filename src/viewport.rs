use crate::camera::{Camera, CellBounds};
use crate::coords::{MAX_CHUNK_COORD, MIN_CHUNK_COORD};
use crate::{CHUNK_SIDE, ChunkCoord};

pub const MAX_RENDER_TILES: usize = 4096;
pub const DEFAULT_OVERSCAN_CHUNKS: i64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkBounds {
    pub min_x: i64,
    pub min_y: i64,
    pub max_x: i64,
    pub max_y: i64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RenderTileCoord {
    pub x: i64,
    pub y: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPlan {
    pub visible_cells: CellBounds,
    pub chunk_bounds_with_overscan: ChunkBounds,
    pub chunks_per_tile: i64,
    pub tiles: Vec<RenderTileCoord>,
}

pub fn plan_visible_tiles(
    camera: Camera,
    width: f64,
    height: f64,
    overscan_chunks: i64,
) -> Option<RenderPlan> {
    if overscan_chunks < 0 {
        return None;
    }
    let visible_cells = camera.visible_cell_bounds(width, height)?;
    let visible_chunks = ChunkBounds {
        min_x: visible_cells.min_x.div_euclid(CHUNK_SIDE as i64),
        min_y: visible_cells.min_y.div_euclid(CHUNK_SIDE as i64),
        max_x: visible_cells.max_x.div_euclid(CHUNK_SIDE as i64),
        max_y: visible_cells.max_y.div_euclid(CHUNK_SIDE as i64),
    };
    let chunk_bounds_with_overscan = ChunkBounds {
        min_x: visible_chunks
            .min_x
            .saturating_sub(overscan_chunks)
            .max(MIN_CHUNK_COORD),
        min_y: visible_chunks
            .min_y
            .saturating_sub(overscan_chunks)
            .max(MIN_CHUNK_COORD),
        max_x: visible_chunks
            .max_x
            .saturating_add(overscan_chunks)
            .min(MAX_CHUNK_COORD),
        max_y: visible_chunks
            .max_y
            .saturating_add(overscan_chunks)
            .min(MAX_CHUNK_COORD),
    };

    let mut chunks_per_tile = 1_i64;
    let (first_tile_x, last_tile_x, first_tile_y, last_tile_y, tile_count) = loop {
        let first_tile_x = chunk_bounds_with_overscan.min_x.div_euclid(chunks_per_tile);
        let last_tile_x = chunk_bounds_with_overscan.max_x.div_euclid(chunks_per_tile);
        let first_tile_y = chunk_bounds_with_overscan.min_y.div_euclid(chunks_per_tile);
        let last_tile_y = chunk_bounds_with_overscan.max_y.div_euclid(chunks_per_tile);
        let count_x = i128::from(last_tile_x) - i128::from(first_tile_x) + 1;
        let count_y = i128::from(last_tile_y) - i128::from(first_tile_y) + 1;
        let tile_count = count_x * count_y;
        if tile_count <= MAX_RENDER_TILES as i128 {
            break (
                first_tile_x,
                last_tile_x,
                first_tile_y,
                last_tile_y,
                tile_count as usize,
            );
        }
        chunks_per_tile = chunks_per_tile.checked_mul(2)?;
    };

    let mut tiles = Vec::with_capacity(tile_count);
    for y in first_tile_y..=last_tile_y {
        for x in first_tile_x..=last_tile_x {
            tiles.push(RenderTileCoord { x, y });
        }
    }

    Some(RenderPlan {
        visible_cells,
        chunk_bounds_with_overscan,
        chunks_per_tile,
        tiles,
    })
}

impl RenderPlan {
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn chunk_coord_for_tile(&self, tile: RenderTileCoord) -> Option<ChunkCoord> {
        Some((
            tile.x.checked_mul(self.chunks_per_tile)?,
            tile.y.checked_mul(self.chunks_per_tile)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellCoord;

    fn camera(origin_x: i64, origin_y: i64, cell_size: f64) -> Camera {
        Camera::new(
            CellCoord {
                x: origin_x,
                y: origin_y,
            },
            cell_size,
        )
        .unwrap()
    }

    #[test]
    fn includes_visible_cells_and_one_chunk_of_overscan() {
        let plan = plan_visible_tiles(camera(0, 0, 1.0), 64.0, 64.0, 1).unwrap();

        assert_eq!(
            plan.visible_cells,
            CellBounds {
                min_x: 0,
                min_y: 0,
                max_x: 63,
                max_y: 63,
            }
        );
        assert_eq!(
            plan.chunk_bounds_with_overscan,
            ChunkBounds {
                min_x: -1,
                min_y: -1,
                max_x: 1,
                max_y: 1,
            }
        );
        assert_eq!(plan.chunks_per_tile, 1);
        assert_eq!(plan.tile_count(), 9);
    }

    #[test]
    fn finds_chunks_crossed_at_positive_and_negative_boundaries() {
        let positive = plan_visible_tiles(camera(63, 63, 1.0), 2.0, 2.0, 0).unwrap();
        assert_eq!(positive.tile_count(), 4);
        assert_eq!(
            positive.chunk_bounds_with_overscan,
            ChunkBounds {
                min_x: 0,
                min_y: 0,
                max_x: 1,
                max_y: 1,
            }
        );

        let negative = plan_visible_tiles(camera(-65, -65, 1.0), 64.0, 64.0, 0).unwrap();
        assert_eq!(negative.tile_count(), 4);
        assert_eq!(
            negative.chunk_bounds_with_overscan,
            ChunkBounds {
                min_x: -2,
                min_y: -2,
                max_x: -1,
                max_y: -1,
            }
        );
    }

    #[test]
    fn aggregates_tiles_by_the_smallest_power_of_two_that_fits_the_budget() {
        let plan = plan_visible_tiles(camera(0, 0, 1.0), 100_000.0, 100_000.0, 1).unwrap();

        assert_eq!(plan.chunks_per_tile, 32);
        assert!(plan.tile_count() <= MAX_RENDER_TILES);
        assert!(plan.tile_count() > 0);
    }

    #[test]
    fn clips_overscan_to_the_world_chunk_range() {
        let plan = plan_visible_tiles(camera(i64::MIN, i64::MAX, 1.0), 1.0, 1.0, i64::MAX).unwrap();

        assert_eq!(plan.chunk_bounds_with_overscan.min_x, MIN_CHUNK_COORD);
        assert_eq!(plan.chunk_bounds_with_overscan.max_x, MAX_CHUNK_COORD);
        assert!(plan.tile_count() <= MAX_RENDER_TILES);
    }

    #[test]
    fn covers_the_entire_i64_world_without_exceeding_the_tile_cap() {
        let plan = plan_visible_tiles(camera(i64::MIN, i64::MIN, 1.0), 1.0e20, 1.0e20, 0).unwrap();

        assert_eq!(plan.visible_cells.max_x, i64::MAX);
        assert_eq!(plan.visible_cells.max_y, i64::MAX);
        assert_eq!(plan.chunks_per_tile, 1_i64 << 52);
        assert_eq!(plan.tile_count(), MAX_RENDER_TILES);
    }

    #[test]
    fn respects_the_tile_cap_across_zoom_levels_and_coordinate_signs() {
        for origin in [-i64::MAX / 2, -1, 0, i64::MAX / 2] {
            for cell_size in [0.01, 0.5, 1.0, 8.0, 256.0] {
                let plan = plan_visible_tiles(
                    camera(origin, -origin, cell_size),
                    100_000.0,
                    75_000.0,
                    DEFAULT_OVERSCAN_CHUNKS,
                )
                .unwrap();
                assert!(plan.tile_count() <= MAX_RENDER_TILES);
                assert!((plan.chunks_per_tile as u64).is_power_of_two());
            }
        }
    }

    #[test]
    fn rejects_invalid_viewport_dimensions_and_overscan() {
        assert!(plan_visible_tiles(camera(0, 0, 1.0), 0.0, 10.0, 0).is_none());
        assert!(plan_visible_tiles(camera(0, 0, 1.0), 10.0, f64::NAN, 0).is_none());
        assert!(plan_visible_tiles(camera(0, 0, 1.0), 10.0, 10.0, -1).is_none());
    }
}
