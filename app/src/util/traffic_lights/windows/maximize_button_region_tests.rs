use warpui::ZoomFactor;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::vec2f;

use super::window_bounds;

#[test]
fn bounds_are_unchanged_at_full_zoom() {
    let bounds = RectF::new(vec2f(3748., 0.), vec2f(46., 35.));
    assert_eq!(window_bounds(bounds, ZoomFactor::new(1.)), bounds);
}

#[test]
fn bounds_follow_the_zoom_into_window_pixels() {
    // At 90% zoom a 3840px window is laid out as if it were 4266.7px wide, so the right-aligned
    // button sits past the real window edge until the zoom is applied.
    let bounds = RectF::new(vec2f(4175., 0.), vec2f(46., 35.));
    let zoomed = window_bounds(bounds, ZoomFactor::new(0.9));
    assert!((zoomed.min_x() - 3757.5).abs() < 0.01);
    assert!((zoomed.max_x() - 3798.9).abs() < 0.01);
    assert!((zoomed.max_y() - 31.5).abs() < 0.01);
    assert!(zoomed.max_x() <= 3840.);
}
