//! Reports where the app-drawn maximize button was painted.
//!
//! Windows answers its non-client hit test from those bounds, which is what opens the Snap Layouts
//! flyout over the button. The bounds have to come from paint: the button is right-aligned, so it
//! moves on every window resize, and a resize re-lays-out the existing element tree without
//! re-running the workspace's `render`.

use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint, WindowId,
};

/// Wraps the maximize button and reports its painted bounds to the platform window.
pub struct MaximizeButtonRegion {
    child: Box<dyn Element>,
    window_id: WindowId,
    /// `false` in fullscreen, where the button leaves fullscreen instead of maximizing and must not
    /// open the flyout.
    reports_bounds: bool,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl MaximizeButtonRegion {
    pub fn new(child: Box<dyn Element>, window_id: WindowId, reports_bounds: bool) -> Self {
        Self {
            child,
            window_id,
            reports_bounds,
            size: None,
            origin: None,
        }
    }
}

impl Element for MaximizeButtonRegion {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);

        let bounds = if self.reports_bounds {
            self.bounds()
        } else {
            None
        };
        report_maximize_button_bounds(self.window_id, bounds, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Publishes the maximize button's bounds, or `None` when the window has no maximize button on
/// screen.
pub fn report_maximize_button_bounds(window_id: WindowId, bounds: Option<RectF>, app: &AppContext) {
    if let Some(window) = app.windows().platform_window(window_id) {
        window.as_ref().set_maximize_button_bounds(bounds);
    }
}
