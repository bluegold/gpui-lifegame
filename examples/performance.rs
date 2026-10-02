use std::time::Instant;

use gpui_lifegame::CellCoord;
use gpui_lifegame::camera::Camera;
use gpui_lifegame::simulation::{next_generation_bit_parallel, next_generation_with_stats};
use gpui_lifegame::viewport::{DEFAULT_OVERSCAN_CHUNKS, plan_visible_tiles};
use gpui_lifegame::world::World;

fn main() {
    benchmark_world("疎", sparse_world(), 5);
    benchmark_world("密", dense_world(), 5);
    benchmark_world("広域分布", broad_world(), 3);
    measure_view_plans();
}

fn benchmark_world(name: &str, mut world: World, generations: usize) {
    let initial_chunks = world.chunk_count();
    let mut reference_total = std::time::Duration::ZERO;
    let mut optimized_total = std::time::Duration::ZERO;
    let mut total_candidates = 0;
    let started = Instant::now();

    for _ in 0..generations {
        let (reference, reference_delta, reference_stats) = next_generation_with_stats(&world);
        let (optimized, optimized_delta, optimized_stats) = next_generation_bit_parallel(&world);
        assert_eq!(optimized, reference, "{name}: optimized world differs");
        assert_eq!(optimized_delta, reference_delta, "{name}: delta differs");
        assert_eq!(
            optimized_stats.candidate_chunks,
            reference_stats.candidate_chunks
        );
        reference_total += reference_stats.elapsed;
        optimized_total += optimized_stats.elapsed;
        total_candidates += optimized_stats.candidate_chunks;
        world = optimized;
    }

    println!(
        "{name}: initial_chunks={initial_chunks}, generations={generations}, avg_candidates={}, reference_ms={:.2}, bit_parallel_ms={:.2}, speedup={:.1}x, elapsed_ms={:.2}, final_chunks={}",
        total_candidates / generations,
        reference_total.as_secs_f64() * 1000.0 / generations as f64,
        optimized_total.as_secs_f64() * 1000.0 / generations as f64,
        reference_total.as_secs_f64() / optimized_total.as_secs_f64(),
        started.elapsed().as_secs_f64() * 1000.0,
        world.chunk_count(),
    );
}

fn sparse_world() -> World {
    let mut world = World::new();
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for chunk_y in -4_i64..4 {
        for chunk_x in -4_i64..4 {
            state = next_random(state);
            let x = chunk_x * 64 + (state & 63) as i64;
            state = next_random(state);
            let y = chunk_y * 64 + (state & 63) as i64;
            world.set(CellCoord { x, y }, true);
        }
    }
    world
}

fn dense_world() -> World {
    let mut world = World::new();
    let mut state = 0x8c67_4f91_2a3d_5be1_u64;
    for y in 0..(4 * 64_i64) {
        for x in 0..(4 * 64_i64) {
            state = next_random(state);
            if state & 3 != 0 {
                world.set(CellCoord { x, y }, true);
            }
        }
    }
    world
}

fn broad_world() -> World {
    let mut world = World::new();
    for chunk_y in -8_i64..=8 {
        for chunk_x in -8_i64..=8 {
            let base_x = chunk_x * 8 * 64;
            let base_y = chunk_y * 8 * 64;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                world.set(
                    CellCoord {
                        x: base_x + dx,
                        y: base_y + dy,
                    },
                    true,
                );
            }
        }
    }
    world
}

fn next_random(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^ (state << 17)
}

fn measure_view_plans() {
    for (name, origin, cell_size) in [("標準倍率", -40, 8.0), ("強ズームアウト", -40, 0.0625)]
    {
        let camera = Camera::new(
            CellCoord {
                x: origin,
                y: origin,
            },
            cell_size,
        )
        .expect("benchmark camera uses a valid scale");
        let started = Instant::now();
        let mut total_tiles = 0;
        for _ in 0..1000 {
            let plan = plan_visible_tiles(camera, 800.0, 552.0, DEFAULT_OVERSCAN_CHUNKS)
                .expect("benchmark viewport is valid");
            total_tiles += plan.tile_count();
        }
        println!(
            "{name}: tiles={}, plan_1000_ms={:.2}",
            total_tiles / 1000,
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }

    let mut camera =
        Camera::new(CellCoord { x: 0, y: 0 }, 8.0).expect("benchmark camera uses a valid scale");
    let started = Instant::now();
    let mut total_tiles = 0;
    for _ in 0..1000 {
        let plan = plan_visible_tiles(camera, 800.0, 552.0, DEFAULT_OVERSCAN_CHUNKS)
            .expect("benchmark viewport is valid");
        total_tiles += plan.tile_count();
        camera.pan_by_pixels(800.0, 552.0);
    }
    println!(
        "パン経路1000回: avg_tiles={}, elapsed_ms={:.2}",
        total_tiles / 1000,
        started.elapsed().as_secs_f64() * 1000.0,
    );
}
