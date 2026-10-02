#[cfg(feature = "desktop")]
pub mod app;
pub mod camera;
pub mod chunk;
pub mod coords;
pub mod simulation;
pub mod viewport;
pub mod world;

pub use coords::{CellCoord, ChunkCoord};

pub const CHUNK_SIDE: usize = 64;
