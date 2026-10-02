use crate::CellCoord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellBounds {
    pub min_x: i64,
    pub min_y: i64,
    pub max_x: i64,
    pub max_y: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPosition {
    cell: CellCoord,
    offset_x: f64,
    offset_y: f64,
}

impl WorldPosition {
    pub fn cell(self) -> CellCoord {
        self.cell
    }

    pub fn offsets(self) -> (f64, f64) {
        (self.offset_x, self.offset_y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    origin_cell: CellCoord,
    offset_x: f64,
    offset_y: f64,
    cell_size: f64,
}

impl Camera {
    pub fn new(origin_cell: CellCoord, cell_size: f64) -> Option<Self> {
        valid_cell_size(cell_size).then_some(Self {
            origin_cell,
            offset_x: 0.0,
            offset_y: 0.0,
            cell_size,
        })
    }

    pub fn origin_cell(self) -> CellCoord {
        self.origin_cell
    }

    pub fn origin_offsets(self) -> (f64, f64) {
        (self.offset_x, self.offset_y)
    }

    pub fn cell_size(self) -> f64 {
        self.cell_size
    }

    pub fn screen_to_world(self, screen_x: f64, screen_y: f64) -> Option<WorldPosition> {
        let (cell_x, offset_x) =
            shift_axis(self.origin_cell.x, self.offset_x, screen_x / self.cell_size)?;
        let (cell_y, offset_y) =
            shift_axis(self.origin_cell.y, self.offset_y, screen_y / self.cell_size)?;

        Some(WorldPosition {
            cell: CellCoord {
                x: cell_x,
                y: cell_y,
            },
            offset_x,
            offset_y,
        })
    }

    pub fn screen_to_cell(self, screen_x: f64, screen_y: f64) -> Option<CellCoord> {
        self.screen_to_world(screen_x, screen_y)
            .map(WorldPosition::cell)
    }

    /// Returns the screen position of the upper-left corner of a cell.
    pub fn cell_to_screen(self, cell: CellCoord) -> Option<(f64, f64)> {
        let x = ((i128::from(cell.x) - i128::from(self.origin_cell.x)) as f64 - self.offset_x)
            * self.cell_size;
        let y = ((i128::from(cell.y) - i128::from(self.origin_cell.y)) as f64 - self.offset_y)
            * self.cell_size;
        (x.is_finite() && y.is_finite()).then_some((x, y))
    }

    pub fn pan_by_pixels(&mut self, delta_x: f64, delta_y: f64) -> bool {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return false;
        }
        let Some((origin_x, offset_x)) =
            shift_axis(self.origin_cell.x, self.offset_x, -delta_x / self.cell_size)
        else {
            return false;
        };
        let Some((origin_y, offset_y)) =
            shift_axis(self.origin_cell.y, self.offset_y, -delta_y / self.cell_size)
        else {
            return false;
        };

        self.origin_cell = CellCoord {
            x: origin_x,
            y: origin_y,
        };
        self.offset_x = offset_x;
        self.offset_y = offset_y;
        true
    }

    pub fn zoom_about(&mut self, screen_x: f64, screen_y: f64, new_cell_size: f64) -> bool {
        if !valid_cell_size(new_cell_size) {
            return false;
        }
        let Some(anchor) = self.screen_to_world(screen_x, screen_y) else {
            return false;
        };
        let Some((origin_x, offset_x)) =
            shift_axis(anchor.cell.x, anchor.offset_x, -screen_x / new_cell_size)
        else {
            return false;
        };
        let Some((origin_y, offset_y)) =
            shift_axis(anchor.cell.y, anchor.offset_y, -screen_y / new_cell_size)
        else {
            return false;
        };

        self.origin_cell = CellCoord {
            x: origin_x,
            y: origin_y,
        };
        self.offset_x = offset_x;
        self.offset_y = offset_y;
        self.cell_size = new_cell_size;
        true
    }

    pub fn visible_cell_bounds(self, width: f64, height: f64) -> Option<CellBounds> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return None;
        }

        Some(CellBounds {
            min_x: self.origin_cell.x,
            min_y: self.origin_cell.y,
            max_x: last_cell_in_view(self.origin_cell.x, self.offset_x, width, self.cell_size),
            max_y: last_cell_in_view(self.origin_cell.y, self.offset_y, height, self.cell_size),
        })
    }
}

fn valid_cell_size(cell_size: f64) -> bool {
    cell_size.is_finite() && cell_size > 0.0
}

fn shift_axis(origin_cell: i64, origin_offset: f64, delta: f64) -> Option<(i64, f64)> {
    if !origin_offset.is_finite() || !(0.0..1.0).contains(&origin_offset) || !delta.is_finite() {
        return None;
    }

    let whole_float = delta.floor();
    let i128_limit = -(i128::MIN as f64);
    if whole_float < i128::MIN as f64 || whole_float >= i128_limit {
        return None;
    }
    let whole = whole_float as i128;
    let fraction = delta - whole_float + origin_offset;
    let carry = fraction.floor() as i128;
    let cell = i128::from(origin_cell)
        .checked_add(whole)?
        .checked_add(carry)?;
    if cell < i128::from(i64::MIN) || cell > i128::from(i64::MAX) {
        return None;
    }

    Some((cell as i64, fraction - carry as f64))
}

fn last_cell_in_view(origin_cell: i64, origin_offset: f64, pixels: f64, cell_size: f64) -> i64 {
    let span = origin_offset + pixels / cell_size;
    let remaining = (i128::from(i64::MAX) - i128::from(origin_cell) + 1) as f64;
    if !span.is_finite() || span >= remaining {
        return i64::MAX;
    }

    let covered_cells = span.ceil() as i128;
    (i128::from(origin_cell) + covered_cells - 1) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn maps_screen_positions_to_cells_using_floor_for_negative_coordinates() {
        let camera = camera(-65, -65, 2.0);

        assert_eq!(
            camera.screen_to_cell(0.0, 0.0),
            Some(CellCoord { x: -65, y: -65 })
        );
        assert_eq!(
            camera.screen_to_cell(127.0, 129.0),
            Some(CellCoord { x: -2, y: -1 })
        );
        assert_eq!(
            camera.cell_to_screen(CellCoord { x: -64, y: -64 }),
            Some((2.0, 2.0))
        );
    }

    #[test]
    fn retains_cell_precision_near_i64_limits() {
        let camera = camera(i64::MAX - 10, i64::MIN + 10, 1.0);

        assert_eq!(
            camera.screen_to_cell(1.0, 1.0),
            Some(CellCoord {
                x: i64::MAX - 9,
                y: i64::MIN + 11,
            })
        );
        assert_eq!(
            camera.cell_to_screen(CellCoord {
                x: i64::MAX - 9,
                y: i64::MIN + 11
            }),
            Some((1.0, 1.0))
        );
    }

    #[test]
    fn pan_moves_the_world_origin_opposite_to_the_drag() {
        let mut camera = camera(10, -10, 2.0);

        assert!(camera.pan_by_pixels(4.0, -6.0));

        assert_eq!(camera.origin_cell(), CellCoord { x: 8, y: -7 });
    }

    #[test]
    fn zoom_keeps_the_world_position_under_the_cursor() {
        let mut camera = camera(100, -5, 10.0);
        let anchor_before = camera.screen_to_world(25.0, 10.0).unwrap();

        assert!(camera.zoom_about(25.0, 10.0, 5.0));

        let anchor_after = camera.screen_to_world(25.0, 10.0).unwrap();
        assert_eq!(anchor_after, anchor_before);
    }

    #[test]
    fn rejects_invalid_scales_and_out_of_world_positions() {
        assert!(Camera::new(CellCoord { x: 0, y: 0 }, 0.0).is_none());
        let camera = camera(i64::MAX, i64::MIN, 1.0);

        assert_eq!(camera.screen_to_cell(1.0, 0.0), None);
        assert_eq!(camera.screen_to_cell(0.0, -1.0), None);
        assert!(camera.visible_cell_bounds(0.0, 1.0).is_none());
    }

    #[test]
    fn clips_visible_bounds_at_the_i64_maximum() {
        let camera = camera(i64::MAX - 1, 0, 0.5);

        assert_eq!(
            camera.visible_cell_bounds(10.0, 1.0),
            Some(CellBounds {
                min_x: i64::MAX - 1,
                min_y: 0,
                max_x: i64::MAX,
                max_y: 1,
            })
        );
    }
}
