use crate::CHUNK_SIDE;

pub type ChunkCoord = (i64, i64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CellCoord {
    pub x: i64,
    pub y: i64,
}

pub const MIN_CHUNK_COORD: i64 = i64::MIN / CHUNK_SIDE as i64;
pub const MAX_CHUNK_COORD: i64 = i64::MAX / CHUNK_SIDE as i64;

pub fn split_cell_coord(cell: CellCoord) -> (ChunkCoord, u8, u8) {
    let side = CHUNK_SIDE as i64;
    let chunk_x = cell.x.div_euclid(side);
    let chunk_y = cell.y.div_euclid(side);
    let local_x = cell.x.rem_euclid(side) as u8;
    let local_y = cell.y.rem_euclid(side) as u8;

    ((chunk_x, chunk_y), local_x, local_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_negative_and_positive_coordinates_to_chunks() {
        let cases = [
            (-65, -2, 63),
            (-64, -1, 0),
            (-1, -1, 63),
            (0, 0, 0),
            (63, 0, 63),
            (64, 1, 0),
        ];

        for (cell_x, expected_chunk_x, expected_local_x) in cases {
            let (chunk, local_x, local_y) = split_cell_coord(CellCoord { x: cell_x, y: 0 });

            assert_eq!(chunk, (expected_chunk_x, 0));
            assert_eq!(local_x, expected_local_x);
            assert_eq!(local_y, 0);
        }
    }

    #[test]
    fn maps_i64_boundaries_without_overflow() {
        let (min_chunk, min_x, min_y) = split_cell_coord(CellCoord {
            x: i64::MIN,
            y: i64::MIN,
        });
        assert_eq!(min_chunk, (MIN_CHUNK_COORD, MIN_CHUNK_COORD));
        assert_eq!((min_x, min_y), (0, 0));

        let (max_chunk, max_x, max_y) = split_cell_coord(CellCoord {
            x: i64::MAX,
            y: i64::MAX,
        });
        assert_eq!(max_chunk, (MAX_CHUNK_COORD, MAX_CHUNK_COORD));
        assert_eq!((max_x, max_y), (63, 63));
    }
}
