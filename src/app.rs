use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, AppContext, Application, Bounds, Context, Entity, IntoElement, ParentElement,
    Render, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};

use crate::CellCoord;
use crate::camera::Camera;
use crate::chunk::Chunk;
use crate::tile_view::{PaintItem, TileView};
use crate::viewport::{DEFAULT_OVERSCAN_CHUNKS, RenderPlan, RenderTileCoord, plan_visible_tiles};
use crate::world::World;

struct AppView {
    world: World,
    camera: Camera,
    tile_views: HashMap<RenderTileCoord, Entity<TileView>>,
}

impl AppView {
    fn new() -> Self {
        let mut world = World::new();
        for (x, y) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            world.set(CellCoord { x, y }, true);
        }
        Self {
            world,
            camera: Camera::new(CellCoord { x: -40, y: -30 }, 8.0)
                .expect("initial camera scale is valid"),
            tile_views: HashMap::new(),
        }
    }

    fn prepare_tiles(&mut self, plan: &RenderPlan, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let desired: HashSet<_> = plan.tiles.iter().copied().collect();
        let mut tile_contents = HashMap::new();
        let grouped = plan.chunks_per_tile > 1;

        for (chunk_coord, chunk) in self.world.chunks() {
            let tile_coord = RenderTileCoord {
                x: chunk_coord.0.div_euclid(plan.chunks_per_tile),
                y: chunk_coord.1.div_euclid(plan.chunks_per_tile),
            };
            if !desired.contains(&tile_coord) {
                continue;
            }
            if grouped {
                let content = tile_contents
                    .entry(tile_coord)
                    .or_insert_with(TileContent::default);
                content.population += f64::from(chunk.population());
            } else {
                tile_contents.insert(
                    tile_coord,
                    TileContent::from_chunk(chunk, self.camera.cell_size()),
                );
            }
        }

        let cell_size = self.camera.cell_size();
        let group_cell_span = plan.chunks_per_tile as f64 * crate::CHUNK_SIDE as f64 * cell_size;
        let mut active = Vec::with_capacity(plan.tiles.len());

        for tile_coord in &plan.tiles {
            let chunk_origin_x = i128::from(tile_coord.x)
                * i128::from(plan.chunks_per_tile)
                * i128::from(crate::CHUNK_SIDE as i64);
            let chunk_origin_y = i128::from(tile_coord.y)
                * i128::from(plan.chunks_per_tile)
                * i128::from(crate::CHUNK_SIDE as i64);
            let x = ((chunk_origin_x - i128::from(self.camera.origin_cell().x)) as f64
                - self.camera.origin_offsets().0)
                * cell_size;
            let y = ((chunk_origin_y - i128::from(self.camera.origin_cell().y)) as f64
                - self.camera.origin_offsets().1)
                * cell_size;
            let tile_width = if grouped {
                group_cell_span
            } else {
                64.0 * cell_size
            };
            let tile_height = tile_width;
            let mut content = tile_contents.remove(tile_coord).unwrap_or_default();
            if grouped {
                let chunk_area = plan.chunks_per_tile as f64 * plan.chunks_per_tile as f64;
                content.items = vec![PaintItem {
                    x: 0.0,
                    y: 0.0,
                    width: tile_width as f32,
                    height: tile_height as f32,
                    density: content.population / (chunk_area * 4096.0),
                }];
            }

            let tile = self
                .tile_views
                .entry(*tile_coord)
                .or_insert_with(|| cx.new(|_| TileView::default()))
                .clone();
            let changed = tile.update(cx, |view, _| view.replace(content.items));
            if changed {
                tile.update(cx, |_, tile_cx| tile_cx.notify());
            }
            active.push(
                div()
                    .absolute()
                    .left(px(x as f32))
                    .top(px(y as f32))
                    .w(px(tile_width as f32))
                    .h(px(tile_height as f32))
                    .border_1()
                    .border_color(rgb(0x263444))
                    .child(tile)
                    .into_any_element(),
            );
        }

        self.tile_views.retain(|coord, _| desired.contains(coord));
        active
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.bounds();
        let width = f64::from(bounds.size.width);
        let height = f64::from(bounds.size.height);
        let tiles = plan_visible_tiles(self.camera, width, height, DEFAULT_OVERSCAN_CHUNKS)
            .map(|plan| self.prepare_tiles(&plan, cx))
            .unwrap_or_default();

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x111827))
            .children(tiles)
    }
}

pub fn run() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.0), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| AppView::new()),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}

#[derive(Default)]
struct TileContent {
    population: f64,
    items: Vec<PaintItem>,
}

impl TileContent {
    fn from_chunk(chunk: &Chunk, cell_size: f64) -> Self {
        let mut content = Self::default();
        if cell_size >= 1.0 {
            for y in 0..64_u8 {
                let mut row = chunk.rows()[usize::from(y)];
                while row != 0 {
                    let x = row.trailing_zeros() as u8;
                    content.items.push(PaintItem {
                        x: f32::from(x) * cell_size as f32,
                        y: f32::from(y) * cell_size as f32,
                        width: cell_size as f32,
                        height: cell_size as f32,
                        density: 1.0,
                    });
                    row &= row - 1;
                }
            }
        } else if cell_size >= 0.125 {
            for block_y in 0..16_u8 {
                for block_x in 0..16_u8 {
                    let mut population = 0_u32;
                    for local_y in block_y * 4..block_y * 4 + 4 {
                        for local_x in block_x * 4..block_x * 4 + 4 {
                            population += u32::from(chunk.get(local_x, local_y));
                        }
                    }
                    if population > 0 {
                        let span = 4.0 * cell_size as f32;
                        content.items.push(PaintItem {
                            x: f32::from(block_x) * span,
                            y: f32::from(block_y) * span,
                            width: span,
                            height: span,
                            density: f64::from(population) / 16.0,
                        });
                    }
                }
            }
        } else if chunk.population() > 0 {
            content.items.push(PaintItem {
                x: 0.0,
                y: 0.0,
                width: 64.0 * cell_size as f32,
                height: 64.0 * cell_size as f32,
                density: f64::from(chunk.population()) / 4096.0,
            });
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_individual_cells_at_readable_scale() {
        let mut chunk = Chunk::default();
        chunk.set(2, 3, true);

        let content = TileContent::from_chunk(&chunk, 2.0);

        assert_eq!(content.items.len(), 1);
        assert_eq!(content.items[0].x, 4.0);
        assert_eq!(content.items[0].y, 6.0);
        assert_eq!(content.items[0].width, 2.0);
        assert_eq!(content.items[0].density, 1.0);
    }

    #[test]
    fn aggregates_cells_into_four_by_four_blocks() {
        let mut chunk = Chunk::default();
        chunk.set(0, 0, true);
        chunk.set(3, 3, true);
        chunk.set(4, 0, true);

        let content = TileContent::from_chunk(&chunk, 0.5);

        assert_eq!(content.items.len(), 2);
        assert_eq!(content.items[0].density, 2.0 / 16.0);
        assert_eq!(content.items[1].x, 2.0);
        assert_eq!(content.items[1].density, 1.0 / 16.0);
    }

    #[test]
    fn shows_chunk_population_as_density_at_small_scale() {
        let mut chunk = Chunk::default();
        chunk.set(0, 0, true);
        chunk.set(1, 0, true);

        let content = TileContent::from_chunk(&chunk, 0.0625);

        assert_eq!(content.items.len(), 1);
        assert_eq!(content.items[0].width, 4.0);
        assert_eq!(content.items[0].density, 2.0 / 4096.0);
    }
}
