use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, ChildAnchor, ChildView, Container, OffsetPositioning, ParentAnchor, ParentOffsetBounds,
    Stack,
};
use warpui::keymap::{FixedBinding, Keystroke};
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::session_configs::SessionOwner;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{
    ActionButton, KeystrokeSource, NakedTheme, PrimaryTheme, SecondaryTheme,
};

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([
        FixedBinding::new(
            "escape",
            SessionInUseAction::Cancel,
            id!(SessionInUseDialog::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            SessionInUseAction::Clone,
            id!(SessionInUseDialog::ui_name()),
        ),
    ]);
}

const DIALOG_WIDTH: f32 = 480.;

pub(crate) enum SessionInUseEvent {
    Clone,
    OpenFromTemplate,
    Cancel,
}

#[derive(Debug)]
pub(crate) enum SessionInUseAction {
    Clone,
    OpenFromTemplate,
    Cancel,
}

pub(crate) struct SessionInUseDialog {
    cancel_button: ViewHandle<ActionButton>,
    template_button: ViewHandle<ActionButton>,
    clone_button: ViewHandle<ActionButton>,
    session_name: String,
    owner: Option<SessionOwner>,
}

impl SessionInUseDialog {
    #[expect(dead_code, reason = "hosted by the workspace in a later task")]
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", NakedTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(SessionInUseAction::Cancel);
            })
        });
        let template_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Open from Template", SecondaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(SessionInUseAction::OpenFromTemplate);
            })
        });
        let enter_keystroke = Keystroke::parse("enter").expect("Valid keystroke");
        let clone_button = ctx.add_typed_action_view(|ctx| {
            ActionButton::new("Clone Session", PrimaryTheme)
                .with_keybinding(KeystrokeSource::Fixed(enter_keystroke), ctx)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(SessionInUseAction::Clone);
                })
        });

        Self {
            cancel_button,
            template_button,
            clone_button,
            session_name: String::new(),
            owner: None,
        }
    }

    #[expect(dead_code, reason = "hosted by the workspace in a later task")]
    pub fn set_session(&mut self, name: String, owner: Option<SessionOwner>) {
        self.session_name = name;
        self.owner = owner;
    }

    fn body(&self) -> String {
        let holder = match &self.owner {
            Some(owner) => {
                let claimed_at = chrono::DateTime::from_timestamp(owner.claimed_at_unix_seconds, 0)
                    .map(|time| {
                        chrono::DateTime::<chrono::Local>::from(time)
                            .format("%b %d at %H:%M")
                            .to_string()
                    });
                match claimed_at {
                    Some(claimed_at) => format!(
                        "Held by {} process {}, since {claimed_at}.",
                        owner.app_id, owner.pid
                    ),
                    None => format!("Held by {} process {}.", owner.app_id, owner.pid),
                }
            }
            None => "Held by another Warp process.".to_owned(),
        };
        format!(
            "{holder}\n\nClone copies the layout, working directories and shells. Running \
             processes, scrollback and agent conversations stay with the original window."
        )
    }
}

impl Entity for SessionInUseDialog {
    type Event = SessionInUseEvent;
}

impl View for SessionInUseDialog {
    fn ui_name() -> &'static str {
        "SessionInUseDialog"
    }

    fn on_focus(&mut self, _focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        let cancel_button = Container::new(ChildView::new(&self.cancel_button).finish())
            .with_margin_right(12.)
            .finish();
        let template_button = Container::new(ChildView::new(&self.template_button).finish())
            .with_margin_right(12.)
            .finish();

        let dialog = Dialog::new(
            format!("'{}' is already open", self.session_name),
            Some(self.body()),
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_bottom_row_child(cancel_button)
        .with_bottom_row_child(template_button)
        .with_bottom_row_child(ChildView::new(&self.clone_button).finish())
        .build()
        .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            dialog,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl TypedActionView for SessionInUseDialog {
    type Action = SessionInUseAction;

    fn handle_action(&mut self, action: &SessionInUseAction, ctx: &mut ViewContext<Self>) {
        match action {
            SessionInUseAction::Clone => ctx.emit(SessionInUseEvent::Clone),
            SessionInUseAction::OpenFromTemplate => ctx.emit(SessionInUseEvent::OpenFromTemplate),
            SessionInUseAction::Cancel => ctx.emit(SessionInUseEvent::Cancel),
        }
    }
}
