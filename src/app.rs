use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext, Application, Bounds, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, Timer,
    Window, WindowBounds, WindowOptions, div, px, rgb, size,
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
    pointer_mode: Option<PointerMode>,
    running: bool,
    timer_task: Option<Task<()>>,
    speed: u16,
    generation: u64,
}

#[derive(Clone, Copy)]
enum PointerMode {
    Painting {
        alive: bool,
        last_cell: CellCoord,
    },
    Panning {
        last_position: gpui::Point<gpui::Pixels>,
    },
}

const MIN_CELL_SIZE: f64 = 0.0625;
const MAX_CELL_SIZE: f64 = 128.0;
const PIXELS_PER_SCROLL_LINE: f64 = 40.0;
const TOOLBAR_HEIGHT: f64 = 48.0;

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
            pointer_mode: None,
            running: false,
            timer_task: None,
            speed: 10,
            generation: 0,
        }
    }

    fn toggle_running(&mut self, cx: &mut Context<Self>) {
        if self.running {
            self.pause(cx);
        } else {
            self.start(cx);
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.running = true;
        let timer_task = cx.spawn(async move |this, cx| {
            loop {
                let Some(speed) = this
                    .update(cx, |view, _| view.running.then_some(view.speed))
                    .ok()
                    .flatten()
                else {
                    break;
                };
                Timer::after(generation_period(speed)).await;
                let should_continue = this
                    .update(cx, |view, cx| {
                        if !view.running {
                            return false;
                        }
                        view.step(cx);
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        });
        self.timer_task = Some(timer_task);
        cx.notify();
    }

    fn pause(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        self.timer_task = None;
        cx.notify();
    }

    fn step(&mut self, cx: &mut Context<Self>) {
        self.world = crate::simulation::next_generation(&self.world).0;
        self.generation = self.generation.saturating_add(1);
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.pause_without_notify();
        self.world = World::new();
        self.generation = 0;
        cx.notify();
    }

    fn randomize(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.pause_without_notify();
        let viewport_size = window.viewport_size();
        let center = self.camera.screen_to_cell(
            f64::from(viewport_size.width) / 2.0,
            (f64::from(viewport_size.height) - TOOLBAR_HEIGHT) / 2.0,
        );
        if let Some(center) = center {
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            self.world = randomized_world(center, seed);
            self.generation = 0;
        }
        cx.notify();
    }

    fn pause_without_notify(&mut self) {
        self.running = false;
        self.timer_task = None;
    }

    fn change_speed(&mut self, increase: bool, cx: &mut Context<Self>) {
        let next = if increase {
            self.speed.saturating_add(5).min(120)
        } else {
            self.speed.saturating_sub(5).max(1)
        };
        if next != self.speed {
            self.speed = next;
            if self.running {
                self.running = false;
                self.timer_task = None;
                self.start(cx);
            } else {
                cx.notify();
            }
        }
    }

    fn begin_paint(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(cell) = self.screen_to_cell(event.position) else {
            return;
        };
        let alive = !self.world.get(cell);
        self.pointer_mode = Some(PointerMode::Painting {
            alive,
            last_cell: cell,
        });
        if self.world.set(cell, alive) {
            cx.notify();
        }
    }

    fn begin_pan(&mut self, event: &MouseDownEvent) {
        self.pointer_mode = Some(PointerMode::Panning {
            last_position: event.position,
        });
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        match self.pointer_mode {
            Some(PointerMode::Painting { alive, last_cell })
                if event.pressed_button == Some(MouseButton::Left) =>
            {
                let Some(cell) = self.screen_to_cell(event.position) else {
                    return;
                };
                let changed = paint_line(&mut self.world, last_cell, cell, alive);
                self.pointer_mode = Some(PointerMode::Painting {
                    alive,
                    last_cell: cell,
                });
                if changed {
                    cx.notify();
                }
            }
            Some(PointerMode::Panning { last_position })
                if event.pressed_button == Some(MouseButton::Middle) =>
            {
                let delta_x = f64::from(event.position.x - last_position.x);
                let delta_y = f64::from(event.position.y - last_position.y);
                self.pointer_mode = Some(PointerMode::Panning {
                    last_position: event.position,
                });
                if self.camera.pan_by_pixels(delta_x, delta_y) {
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn end_pointer(&mut self, button: MouseButton) {
        let should_end = matches!(
            (self.pointer_mode, button),
            (Some(PointerMode::Painting { .. }), MouseButton::Left)
                | (Some(PointerMode::Panning { .. }), MouseButton::Middle)
        );
        if should_end {
            self.pointer_mode = None;
        }
    }

    fn zoom(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let scroll_lines = match event.delta {
            ScrollDelta::Lines(delta) => f64::from(delta.y),
            ScrollDelta::Pixels(delta) => f64::from(delta.y) / PIXELS_PER_SCROLL_LINE,
        };
        if !scroll_lines.is_finite() || scroll_lines == 0.0 {
            return;
        }
        let new_cell_size = zoomed_cell_size(self.camera.cell_size(), scroll_lines);
        if new_cell_size != self.camera.cell_size()
            && self.camera.zoom_about(
                f64::from(event.position.x),
                f64::from(event.position.y) - TOOLBAR_HEIGHT,
                new_cell_size,
            )
        {
            cx.notify();
        }
    }

    fn screen_to_cell(&self, position: gpui::Point<gpui::Pixels>) -> Option<CellCoord> {
        self.camera.screen_to_cell(
            f64::from(position.x),
            f64::from(position.y) - TOOLBAR_HEIGHT,
        )
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
        let viewport_size = window.viewport_size();
        let width = f64::from(viewport_size.width);
        let height = (f64::from(viewport_size.height) - TOOLBAR_HEIGHT).max(1.0);
        let tiles = plan_visible_tiles(self.camera, width, height, DEFAULT_OVERSCAN_CHUNKS)
            .map(|plan| self.prepare_tiles(&plan, cx))
            .unwrap_or_default();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x111827))
            .child(
                div()
                    .h(px(TOOLBAR_HEIGHT as f32))
                    .w_full()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .bg(rgb(0x1f2937))
                    .child(control_button(
                        "run-toggle",
                        if self.running { "Pause" } else { "Start" },
                        cx.listener(|this, _, _, cx| this.toggle_running(cx)),
                    ))
                    .child(control_button(
                        "step",
                        "Step",
                        cx.listener(|this, _, _, cx| this.step(cx)),
                    ))
                    .child(control_button(
                        "clear",
                        "Clear",
                        cx.listener(|this, _, _, cx| this.clear(cx)),
                    ))
                    .child(control_button(
                        "randomize",
                        "Randomize",
                        cx.listener(|this, _, window, cx| this.randomize(window, cx)),
                    ))
                    .child(
                        div()
                            .text_color(gpui::white())
                            .child(format!("世代 {}", self.generation)),
                    )
                    .child(
                        div()
                            .text_color(gpui::white())
                            .child(format!("{} 世代/秒", self.speed)),
                    )
                    .child(control_button(
                        "speed-down",
                        "−",
                        cx.listener(|this, _, _, cx| this.change_speed(false, cx)),
                    ))
                    .child(control_button(
                        "speed-up",
                        "+",
                        cx.listener(|this, _, _, cx| this.change_speed(true, cx)),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .bg(rgb(0x111827))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_paint(event, cx)
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseDownEvent, _, _| this.begin_pan(event)),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            this.end_pointer(MouseButton::Left)
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            this.end_pointer(MouseButton::Left)
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            this.end_pointer(MouseButton::Middle)
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Middle,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            this.end_pointer(MouseButton::Middle)
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        this.pointer_move(event, cx)
                    }))
                    .on_scroll_wheel(
                        cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.zoom(event, cx)),
                    )
                    .children(tiles),
            )
    }
}

fn control_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .flex_none()
        .px_3()
        .py_1()
        .bg(rgb(0x374151))
        .text_color(gpui::white())
        .rounded_sm()
        .cursor_pointer()
        .child(label)
        .on_click(on_click)
}

fn randomized_world(center: CellCoord, mut state: u64) -> World {
    if state == 0 {
        state = 0x9e37_79b9_7f4a_7c15;
    }
    let mut world = World::new();
    for y in 0..64_i128 {
        for x in 0..64_i128 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if state & 0b11 == 0 {
                let Some(cell_x) = i64::try_from(i128::from(center.x) + x - 32).ok() else {
                    continue;
                };
                let Some(cell_y) = i64::try_from(i128::from(center.y) + y - 32).ok() else {
                    continue;
                };
                world.set(
                    CellCoord {
                        x: cell_x,
                        y: cell_y,
                    },
                    true,
                );
            }
        }
    }
    world
}

fn generation_period(speed: u16) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(speed.clamp(1, 120)))
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

fn paint_line(world: &mut World, start: CellCoord, end: CellCoord, alive: bool) -> bool {
    let mut x = i128::from(start.x);
    let mut y = i128::from(start.y);
    let end_x = i128::from(end.x);
    let end_y = i128::from(end.y);
    let delta_x = (end_x - x).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let delta_y = -(end_y - y).abs();
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = delta_x + delta_y;
    let mut changed = false;

    loop {
        changed |= world.set(
            CellCoord {
                x: x as i64,
                y: y as i64,
            },
            alive,
        );
        if x == end_x && y == end_y {
            break;
        }
        let twice_error = 2 * error;
        if twice_error >= delta_y {
            error += delta_y;
            x += step_x;
        }
        if twice_error <= delta_x {
            error += delta_x;
            y += step_y;
        }
    }
    changed
}

fn zoomed_cell_size(current: f64, scroll_lines: f64) -> f64 {
    (current * 1.1_f64.powf(-scroll_lines.clamp(-100.0, 100.0))).clamp(MIN_CELL_SIZE, MAX_CELL_SIZE)
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

    #[test]
    fn drag_paint_fills_each_cell_between_pointer_events() {
        let mut world = World::new();
        let start = CellCoord { x: -3, y: -2 };
        let end = CellCoord { x: 2, y: 1 };

        assert!(paint_line(&mut world, start, end, true));
        assert!(world.get(start));
        assert!(world.get(end));
        assert!(world.get(CellCoord { x: -1, y: -1 }));
        assert!(!paint_line(&mut world, start, end, true));
    }

    #[test]
    fn wheel_zoom_is_bounded_and_has_the_expected_direction() {
        assert!(zoomed_cell_size(8.0, 1.0) < 8.0);
        assert!(zoomed_cell_size(8.0, -1.0) > 8.0);
        assert_eq!(zoomed_cell_size(8.0, 100.0), MIN_CELL_SIZE);
        assert_eq!(zoomed_cell_size(8.0, -100.0), MAX_CELL_SIZE);
    }

    #[test]
    fn randomize_is_repeatable_for_a_seed_and_fills_the_world() {
        let center = CellCoord { x: -10, y: 20 };
        let first = randomized_world(center, 1234);
        let second = randomized_world(center, 1234);
        let different = randomized_world(center, 5678);

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert!(!first.is_empty());
    }

    #[test]
    fn simulation_period_stays_within_the_configured_speed_range() {
        assert_eq!(generation_period(1), Duration::from_secs(1));
        assert!(generation_period(120) >= Duration::from_millis(8));
        assert!(generation_period(120) < Duration::from_millis(9));
        assert_eq!(generation_period(0), Duration::from_secs(1));
        assert_eq!(generation_period(121), generation_period(120));
    }
}
