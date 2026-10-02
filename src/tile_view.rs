use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    Context, IntoElement, ParentElement, Render, Styled, Window, canvas, div, fill, point, px, rgb,
    size,
};

static PAINTED_ITEMS: AtomicUsize = AtomicUsize::new(0);
static PAINT_TIME_NANOS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn take_paint_metrics() -> (usize, Duration) {
    (
        PAINTED_ITEMS.swap(0, Ordering::Relaxed),
        Duration::from_nanos(PAINT_TIME_NANOS.swap(0, Ordering::Relaxed)),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaintItem {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub density: f64,
}

#[derive(Default)]
pub struct TileView {
    items: Vec<PaintItem>,
}

impl TileView {
    pub(crate) fn replace(&mut self, items: Vec<PaintItem>) -> bool {
        if self.items == items {
            return false;
        }
        self.items = items;
        true
    }
}

impl Render for TileView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items.clone();
        div().size_full().child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let started = Instant::now();
                    let item_count = items.len();
                    for item in items {
                        let density = item.density.clamp(0.0, 1.0);
                        let red = (17.0 + 55.0 * density) as u32;
                        let green = (24.0 + 181.0 * density) as u32;
                        let blue = (39.0 + 145.0 * density) as u32;
                        window.paint_quad(fill(
                            gpui::Bounds {
                                origin: point(
                                    bounds.origin.x + px(item.x),
                                    bounds.origin.y + px(item.y),
                                ),
                                size: size(px(item.width), px(item.height)),
                            },
                            rgb((red << 16) | (green << 8) | blue),
                        ));
                    }
                    PAINTED_ITEMS.fetch_add(item_count, Ordering::Relaxed);
                    PAINT_TIME_NANOS.fetch_add(
                        started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
                        Ordering::Relaxed,
                    );
                },
            )
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_reports_only_real_content_changes() {
        let item = PaintItem {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
            density: 1.0,
        };
        let mut tile = TileView::default();

        assert!(tile.replace(vec![item.clone()]));
        assert!(!tile.replace(vec![item.clone()]));
        assert!(tile.replace(Vec::new()));
    }
}
