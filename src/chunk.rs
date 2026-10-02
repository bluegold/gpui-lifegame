use crate::CHUNK_SIDE;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    rows: [u64; CHUNK_SIDE],
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            rows: [0; CHUNK_SIDE],
        }
    }
}

impl Chunk {
    pub fn get(&self, x: u8, y: u8) -> bool {
        let (x, y) = local_indices(x, y);
        self.rows[y] & (1_u64 << x) != 0
    }

    pub fn set(&mut self, x: u8, y: u8, alive: bool) {
        let (x, y) = local_indices(x, y);
        let mask = 1_u64 << x;
        if alive {
            self.rows[y] |= mask;
        } else {
            self.rows[y] &= !mask;
        }
    }

    pub fn toggle(&mut self, x: u8, y: u8) {
        let (x, y) = local_indices(x, y);
        self.rows[y] ^= 1_u64 << x;
    }

    pub fn is_empty(&self) -> bool {
        self.rows().iter().all(|row| *row == 0)
    }

    pub fn population(&self) -> u32 {
        self.rows().iter().map(|row| row.count_ones()).sum()
    }

    pub(crate) fn rows(&self) -> &[u64; CHUNK_SIDE] {
        &self.rows
    }

    pub(crate) fn set_row_bits(&mut self, y: usize, row: u64) {
        self.rows[y] = row;
    }
}

fn local_indices(x: u8, y: u8) -> (u32, usize) {
    assert!(
        usize::from(x) < CHUNK_SIDE,
        "local x coordinate must be < 64"
    );
    assert!(
        usize::from(y) < CHUNK_SIDE,
        "local y coordinate must be < 64"
    );
    (u32::from(x), usize::from(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty_and_tracks_cell_state() {
        let mut chunk = Chunk::default();

        assert!(chunk.is_empty());
        assert_eq!(chunk.population(), 0);
        assert!(!chunk.get(0, 0));

        chunk.set(0, 0, true);
        chunk.set(63, 63, true);
        assert!(chunk.get(0, 0));
        assert!(chunk.get(63, 63));
        assert_eq!(chunk.population(), 2);
        assert!(!chunk.is_empty());

        chunk.set(0, 0, false);
        chunk.set(63, 63, false);
        assert!(chunk.is_empty());
        assert_eq!(chunk.population(), 0);
    }

    #[test]
    fn set_is_idempotent_and_toggle_flips_a_cell() {
        let mut chunk = Chunk::default();

        chunk.set(5, 7, true);
        chunk.set(5, 7, true);
        assert_eq!(chunk.population(), 1);

        chunk.toggle(5, 7);
        assert!(!chunk.get(5, 7));
        chunk.toggle(5, 7);
        assert!(chunk.get(5, 7));
    }

    #[test]
    #[should_panic(expected = "local x coordinate must be < 64")]
    fn rejects_out_of_range_local_coordinates() {
        Chunk::default().get(64, 0);
    }

    #[test]
    fn exposes_rows_to_crate_code() {
        let mut chunk = Chunk::default();
        chunk.set(63, 0, true);

        assert_eq!(chunk.rows()[0], 1_u64 << 63);
    }
}
