use std::cell::RefCell;
use std::rc::Rc;

use warpui::platform::WindowStyle;
use warpui::{App, TypedActionView as _};

use super::{Action, Event, ShellTerminatedBanner, TerminationType};
use crate::terminal::model::terminal_model::ExitReason;
use crate::test_util::terminal::initialize_app_for_terminal_view;

#[test]
fn retry_action_emits_retry_event() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);

        let events = Rc::new(RefCell::new(Vec::new()));
        let events_handle = events.clone();
        let (_, banner) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            ShellTerminatedBanner::new(
                TerminationType::Premature {
                    shell_detail: "shell".to_string(),
                    reason: ExitReason::ShellProcessExited,
                },
                ctx,
            )
        });
        app.update(|ctx| {
            ctx.subscribe_to_view(&banner, move |_, event, _| {
                events_handle.borrow_mut().push(*event);
            });
        });

        banner.update(&mut app, |banner, ctx| {
            banner.handle_action(&Action::Retry, ctx);
        });

        assert_eq!(*events.borrow(), vec![Event::Retry]);
    });
}
