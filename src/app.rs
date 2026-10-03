use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::size_of;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext, Application, Bounds, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, Timer,
    Window, WindowBounds, WindowOptions, div, px, rgb, size,
};

use crate::CellCoord;
use crate::camera::Camera;
use crate::chunk::Chunk;
use crate::coords::split_cell_coord;
use crate::tile_view::{PaintItem, TileView};
use crate::viewport::{DEFAULT_OVERSCAN_CHUNKS, RenderPlan, RenderTileCoord, plan_visible_tiles};
use crate::world::World;

struct AppView {
    world: World,
    camera: Camera,
    tile_views: HashMap<RenderTileCoord, Entity<TileView>>,
    tile_content_keys: HashMap<RenderTileCoord, TileContentKey>,
    dirty_chunks: HashSet<crate::ChunkCoord>,
    invalidate_all_tiles: bool,
    pointer_mode: Option<PointerMode>,
    running: bool,
    timer_task: Option<Task<()>>,
    speed: u16,
    generation: u64,
    generation_stats: crate::simulation::GenerationStats,
    render_stats: RenderStats,
    frame_times: VecDeque<Instant>,
    frame_rate: f64,
    metrics_started_at: Instant,
    metric_samples: VecDeque<MetricSample>,
}

#[derive(Clone, Copy, Default)]
struct RenderStats {
    visible_chunks: usize,
    tile_views: usize,
    updated_tiles: usize,
    paint_items: usize,
    prepare_time: Duration,
    delayed_generations: u64,
}

#[derive(Clone, Copy, PartialEq)]
struct TileContentKey {
    cell_size: f64,
    chunks_per_tile: i64,
}

#[derive(Clone, Copy)]
struct MetricSample {
    elapsed_ms: f64,
    frame_rate: f64,
    generation: u64,
    origin_x: i64,
    origin_y: i64,
    cell_size: f64,
    sim_ms: f64,
    candidate_chunks: usize,
    visible_chunks: usize,
    tile_views: usize,
    updated_tiles: usize,
    paint_items: usize,
    painted_items: usize,
    paint_ms: f64,
    prepare_ms: f64,
    delayed_generations: u64,
    world_chunks: usize,
    estimated_bytes: usize,
}

const MAX_METRIC_SAMPLES: usize = 10_000;

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
const TOOLBAR_HEIGHT: f64 = 92.0;

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
            tile_content_keys: HashMap::new(),
            dirty_chunks: HashSet::new(),
            invalidate_all_tiles: true,
            pointer_mode: None,
            running: false,
            timer_task: None,
            speed: 10,
            generation: 0,
            generation_stats: crate::simulation::GenerationStats::default(),
            render_stats: RenderStats::default(),
            frame_times: VecDeque::with_capacity(64),
            frame_rate: 0.0,
            metrics_started_at: Instant::now(),
            metric_samples: VecDeque::with_capacity(MAX_METRIC_SAMPLES),
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
        let (world, delta, stats) = crate::simulation::next_generation_bit_parallel(&self.world);
        self.world = world;
        self.dirty_chunks.extend(delta.changed_chunks);
        self.generation_stats = stats;
        let period = generation_period(self.speed);
        if self.generation_stats.elapsed > period {
            self.render_stats.delayed_generations =
                self.render_stats.delayed_generations.saturating_add(1);
        }
        self.generation = self.generation.saturating_add(1);
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.pause_without_notify();
        self.world = World::new();
        self.dirty_chunks.clear();
        self.invalidate_all_tiles = true;
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
            self.dirty_chunks.clear();
            self.invalidate_all_tiles = true;
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

    fn export_metrics(&self, cx: &mut Context<Self>) {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let receiver = cx.prompt_for_new_path(&directory, Some("lifegame-performance.csv"));
        let samples = self.metric_samples.iter().copied().collect::<Vec<_>>();
        cx.spawn(async move |_this, _cx| match receiver.await {
            Ok(Ok(Some(mut path))) => {
                if path.extension().is_none() {
                    path.set_extension("csv");
                }
                match write_metrics_csv(&path, &samples) {
                    Ok(()) => eprintln!(
                        "計測CSVを保存しました: {} ({}件)",
                        path.display(),
                        samples.len()
                    ),
                    Err(error) => eprintln!("計測CSVの保存に失敗しました: {error}"),
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => eprintln!("保存先ダイアログを開けませんでした: {error}"),
            Err(error) => eprintln!("保存先ダイアログが終了しました: {error}"),
        })
        .detach();
    }

    fn record_frame_completion(&mut self) {
        let now = Instant::now();
        self.frame_times.push_back(now);
        while self.frame_times.len() > 2
            && now.duration_since(self.frame_times[0]) > Duration::from_secs(1)
        {
            self.frame_times.pop_front();
        }
        while self.frame_times.len() > 64 {
            self.frame_times.pop_front();
        }
        self.frame_rate = match (self.frame_times.front(), self.frame_times.back()) {
            (Some(first), Some(last)) if self.frame_times.len() > 1 => {
                let elapsed = last.duration_since(*first).as_secs_f64();
                if elapsed > 0.0 {
                    (self.frame_times.len() - 1) as f64 / elapsed
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
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
            self.dirty_chunks.insert(split_cell_coord(cell).0);
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
                    self.dirty_chunks.clear();
                    self.invalidate_all_tiles = true;
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
        let started = Instant::now();
        let desired: HashSet<_> = plan.tiles.iter().copied().collect();
        let cell_size = self.camera.cell_size();
        let content_key = TileContentKey {
            cell_size,
            chunks_per_tile: plan.chunks_per_tile,
        };
        let mut dirty_tiles: HashSet<_> = plan
            .tiles
            .iter()
            .copied()
            .filter(|coord| {
                self.invalidate_all_tiles || self.tile_content_keys.get(coord) != Some(&content_key)
            })
            .collect();
        dirty_tiles.extend(self.dirty_chunks.iter().filter_map(|&(chunk_x, chunk_y)| {
            let tile_coord = RenderTileCoord {
                x: chunk_x.div_euclid(plan.chunks_per_tile),
                y: chunk_y.div_euclid(plan.chunks_per_tile),
            };
            desired.contains(&tile_coord).then_some(tile_coord)
        }));

        let mut tile_contents: HashMap<RenderTileCoord, TileContent> = HashMap::new();
        let grouped = plan.chunks_per_tile > 1;
        let mut visible_chunks = 0;

        for (chunk_coord, chunk) in self.world.chunks() {
            let tile_coord = RenderTileCoord {
                x: chunk_coord.0.div_euclid(plan.chunks_per_tile),
                y: chunk_coord.1.div_euclid(plan.chunks_per_tile),
            };
            if !desired.contains(&tile_coord) {
                continue;
            }
            visible_chunks += 1;
            if !dirty_tiles.contains(&tile_coord) {
                continue;
            }
            if grouped {
                let content = tile_contents
                    .entry(tile_coord)
                    .or_insert_with(TileContent::default);
                content.population += f64::from(chunk.population());
            } else {
                tile_contents.insert(tile_coord, TileContent::from_chunk(chunk, cell_size));
            }
        }

        let group_cell_span = plan.chunks_per_tile as f64 * crate::CHUNK_SIDE as f64 * cell_size;
        let mut active = Vec::with_capacity(plan.tiles.len());
        let mut updated_tiles = 0;
        let mut paint_items = 0;

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
            let tile = self
                .tile_views
                .entry(*tile_coord)
                .or_insert_with(|| cx.new(|_| TileView::default()))
                .clone();
            if dirty_tiles.contains(tile_coord) {
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
                let changed = tile.update(cx, |view, _| view.replace(content.items));
                if changed {
                    updated_tiles += 1;
                    tile.update(cx, |_, tile_cx| tile_cx.notify());
                }
                self.tile_content_keys.insert(*tile_coord, content_key);
            }
            paint_items += tile.read(cx).item_count();
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
        self.tile_content_keys
            .retain(|coord, _| desired.contains(coord));
        self.dirty_chunks.retain(|&(chunk_x, chunk_y)| {
            !desired.contains(&RenderTileCoord {
                x: chunk_x.div_euclid(plan.chunks_per_tile),
                y: chunk_y.div_euclid(plan.chunks_per_tile),
            })
        });
        self.invalidate_all_tiles = false;
        self.render_stats = RenderStats {
            visible_chunks,
            tile_views: self.tile_views.len(),
            updated_tiles,
            paint_items,
            prepare_time: started.elapsed(),
            delayed_generations: self.render_stats.delayed_generations,
        };
        active
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        cx.on_next_frame(window, |view, _, _| view.record_frame_completion());
        let viewport_size = window.viewport_size();
        let width = f64::from(viewport_size.width);
        let height = (f64::from(viewport_size.height) - TOOLBAR_HEIGHT).max(1.0);
        let tiles = plan_visible_tiles(self.camera, width, height, DEFAULT_OVERSCAN_CHUNKS)
            .map(|plan| self.prepare_tiles(&plan, cx))
            .unwrap_or_default();
        let stats = self.render_stats;
        let (painted_items, paint_time) = crate::tile_view::take_paint_metrics();
        let estimated_bytes =
            self.world.chunk_count() * (size_of::<Chunk>() + size_of::<crate::ChunkCoord>() + 32);
        self.record_metrics(stats, painted_items, paint_time, estimated_bytes);

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
                    .flex_col()
                    .flex_none()
                    .bg(rgb(0x1f2937))
                    .child(
                        div()
                            .h(px(48.0))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
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
                            ))
                            .child(control_button(
                                "export-metrics",
                                "CSV",
                                cx.listener(|this, _, _, cx| this.export_metrics(cx)),
                            )),
                    )
                    .child(
                        div()
                            .h(px(44.0))
                            .w_full()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .px_3()
                            .text_color(gpui::white())
                            .child(
                                div()
                                    .h(px(22.0))
                                    .flex()
                                    .items_center()
                                    .child(format!(
                                        "UI FPS {:.1} | sim {:.2} ms / 候補 {} | 表示 {} | View {} | 更新 {} | items {}",
                                        self.frame_rate,
                                        self.generation_stats.elapsed.as_secs_f64() * 1000.0,
                                        self.generation_stats.candidate_chunks,
                                        stats.visible_chunks,
                                        stats.tile_views,
                                        stats.updated_tiles,
                                        stats.paint_items,
                                    )),
                            )
                            .child(
                                div()
                                    .h(px(22.0))
                                    .flex()
                                    .items_center()
                                    .child(format!(
                                        "paint {}・{:.1} ms | 準備 {:.2} ms | 超過 {} | chunk {} / 約 {:.1} KiB",
                                        painted_items,
                                        paint_time.as_secs_f64() * 1000.0,
                                        stats.prepare_time.as_secs_f64() * 1000.0,
                                        stats.delayed_generations,
                                        self.world.chunk_count(),
                                        estimated_bytes as f64 / 1024.0,
                                    )),
                            ),
                    ),
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

impl AppView {
    fn record_metrics(
        &mut self,
        stats: RenderStats,
        painted_items: usize,
        paint_time: Duration,
        estimated_bytes: usize,
    ) {
        if self.metric_samples.len() == MAX_METRIC_SAMPLES {
            self.metric_samples.pop_front();
        }
        self.metric_samples.push_back(MetricSample {
            elapsed_ms: self.metrics_started_at.elapsed().as_secs_f64() * 1000.0,
            frame_rate: self.frame_rate,
            generation: self.generation,
            origin_x: self.camera.origin_cell().x,
            origin_y: self.camera.origin_cell().y,
            cell_size: self.camera.cell_size(),
            sim_ms: self.generation_stats.elapsed.as_secs_f64() * 1000.0,
            candidate_chunks: self.generation_stats.candidate_chunks,
            visible_chunks: stats.visible_chunks,
            tile_views: stats.tile_views,
            updated_tiles: stats.updated_tiles,
            paint_items: stats.paint_items,
            painted_items,
            paint_ms: paint_time.as_secs_f64() * 1000.0,
            prepare_ms: stats.prepare_time.as_secs_f64() * 1000.0,
            delayed_generations: stats.delayed_generations,
            world_chunks: self.world.chunk_count(),
            estimated_bytes,
        });
    }
}

fn write_metrics_csv(path: &std::path::Path, samples: &[MetricSample]) -> std::io::Result<()> {
    let mut csv = String::from(
        "elapsed_ms,ui_fps,generation,camera_origin_x,camera_origin_y,cell_size,sim_ms,candidate_chunks,visible_chunks,tile_views,updated_tiles,paint_items,painted_items,paint_ms,prepare_ms,delayed_generations,world_chunks,estimated_bytes\n",
    );
    for sample in samples {
        use std::fmt::Write as _;
        writeln!(
            csv,
            "{:.3},{:.1},{},{},{},{:.5},{:.3},{},{},{},{},{},{},{:.3},{:.3},{},{},{}",
            sample.elapsed_ms,
            sample.frame_rate,
            sample.generation,
            sample.origin_x,
            sample.origin_y,
            sample.cell_size,
            sample.sim_ms,
            sample.candidate_chunks,
            sample.visible_chunks,
            sample.tile_views,
            sample.updated_tiles,
            sample.paint_items,
            sample.painted_items,
            sample.paint_ms,
            sample.prepare_ms,
            sample.delayed_generations,
            sample.world_chunks,
            sample.estimated_bytes,
        )
        .expect("writing a CSV row to a String cannot fail");
    }
    std::fs::write(path, csv)
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
                    let shift = u32::from(block_x) * 4;
                    let population: u32 = chunk.rows()
                        [usize::from(block_y) * 4..usize::from(block_y) * 4 + 4]
                        .iter()
                        .map(|row| ((row >> shift) & 0x0f).count_ones())
                        .sum();
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
