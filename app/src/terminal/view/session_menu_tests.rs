use tempfile::TempDir;
use warpui::App;

use super::*;
use crate::pane_group::BackingView;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

#[cfg(feature = "local_tty")]
#[test]
fn older_supervisor_connection_is_visible_and_restart_clears_its_status() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |view, ctx| {
            view.is_login_shell_bootstrapped = true;
            view.on_supervisor_changed(
                Some(SessionHolder {
                    channel: "older-supervisor".to_owned(),
                    version: "old-build".to_owned(),
                    pid: 42,
                }),
                ctx,
            );
            assert!(
                view.inline_banners_state
                    .connected_supervisor_banner
                    .is_some()
            );
            let items = view.pane_header_overflow_menu_items(ctx);
            assert!(items.iter().any(|item| item.fields().is_some_and(|fields| {
                fields.label() == "Supervisor: build old-build" && fields.is_disabled()
            })));
            assert!(items.iter().any(|item| {
                item.fields().is_some_and(|fields| {
                    fields.label() == "Restart & take over…"
                        && matches!(
                            fields.on_select_action(),
                            Some(TerminalAction::RestartInCurrentSupervisor)
                        )
                })
            }));
            view.on_session_restart_started(ctx);
            assert!(view.connected_supervisor.is_none());
            assert!(
                view.inline_banners_state
                    .connected_supervisor_banner
                    .is_none()
            );
            assert_eq!(view.model.lock().block_list().inline_banner_count(), 0);
        });
    });
}

#[test]
fn single_pane_background_menu_offers_obachan_and_sessions() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let repo = TempDir::new().unwrap();
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(repo.path(), ctx);
            view.set_current_repo_path_for_test(Some(repo.path()));
            assert!(!view.split_pane_state(ctx).is_in_split_pane());
            assert!(view.model.lock().is_block_list_empty());

            let items = view.context_menu_items(
                &BlockListMenuSource::OutsideBlockRightClick {
                    position_in_terminal_view: Vector2F::zero(),
                },
                ctx,
            );
            let start = items
                .iter()
                .filter_map(MenuItem::fields)
                .find(|fields| fields.label() == "Start in Obachan")
                .expect("Obachan must be accessible without splitting the terminal");
            assert!(matches!(
                start.on_select_action(),
                Some(TerminalAction::StartAgentOnAgentCtrl)
            ));
            assert!(!start.is_disabled());
            let sessions = items
                .iter()
                .filter_map(MenuItem::fields)
                .find(|fields| fields.label() == "Sessions…")
                .expect("session recovery must be accessible without the pane header");
            assert!(matches!(
                sessions.on_select_action(),
                Some(TerminalAction::ShowAgentSessions)
            ));
            assert!(!items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::ToggleMaximizePane)
            )));

            let header_items = view.pane_header_overflow_menu_items(ctx);
            assert!(header_items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::StartAgentOnAgentCtrl)
            )));
        });
    });
}

#[test]
fn single_pane_alt_screen_menu_offers_obachan_and_sessions() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let repo = TempDir::new().unwrap();
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(repo.path(), ctx);
            view.set_current_repo_path_for_test(Some(repo.path()));
            view.model.lock().process_bytes("\x1b[?1049h");
            assert!(!view.split_pane_state(ctx).is_in_split_pane());

            let items = view.rebuild_alt_screen_context_menu_items(ctx);
            assert!(items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::StartAgentOnAgentCtrl)
            )));
            assert!(items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::ShowAgentSessions)
            )));
        });
    });
}

#[test]
fn single_pane_input_menu_offers_obachan_and_sessions() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let repo = TempDir::new().unwrap();
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(repo.path(), ctx);
            view.set_current_repo_path_for_test(Some(repo.path()));

            let items = view.input_context_menu_items(ctx);
            assert!(items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::StartAgentOnAgentCtrl)
            )));
            assert!(items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::ShowAgentSessions)
            )));
        });
    });
}

#[test]
fn background_menu_outside_a_repo_disables_obachan() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let directory = TempDir::new().unwrap();
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(directory.path(), ctx);
            view.set_current_repo_path_for_test(None);
            let items = view.context_menu_items(
                &BlockListMenuSource::OutsideBlockRightClick {
                    position_in_terminal_view: Vector2F::zero(),
                },
                ctx,
            );

            assert!(items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::ShowAgentSessions)
            )));
            let start = items
                .iter()
                .filter_map(MenuItem::fields)
                .find(|fields| fields.label() == "Start in Obachan")
                .expect("Obachan should remain discoverable outside a repository");
            assert!(matches!(
                start.on_select_action(),
                Some(TerminalAction::StartAgentOnAgentCtrl)
            ));
            assert!(start.is_disabled());
            assert_eq!(
                start.tooltip_text(),
                Some("Open a local Git repository in this pane to start in Obachan")
            );
        });
    });
}

#[test]
fn text_selection_menu_does_not_offer_session_management() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let repo = TempDir::new().unwrap();
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(repo.path(), ctx);
            view.set_current_repo_path_for_test(Some(repo.path()));
            view.model.lock().simulate_block("echo hello", "hello");
            let items = view.context_menu_items(
                &BlockListMenuSource::RegularTextRightClick {
                    position_in_terminal_view: Vector2F::zero(),
                },
                ctx,
            );

            assert!(!items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::StartAgentOnAgentCtrl | TerminalAction::ShowAgentSessions)
            )));
        });
    });
}

#[test]
fn shared_viewer_background_menu_does_not_offer_local_sessions() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            view.model
                .lock()
                .set_shared_session_status(SharedSessionStatus::FinishedViewer);
            let items = view.context_menu_items(
                &BlockListMenuSource::OutsideBlockRightClick {
                    position_in_terminal_view: Vector2F::zero(),
                },
                ctx,
            );

            assert!(!items.iter().any(|item| matches!(
                item.fields().and_then(MenuItemFields::on_select_action),
                Some(TerminalAction::StartAgentOnAgentCtrl | TerminalAction::ShowAgentSessions)
            )));
        });
    });
}
