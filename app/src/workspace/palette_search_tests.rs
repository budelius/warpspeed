use warpui::App;

use super::*;
use crate::search::command_palette::SelectedItems;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::settings::import::model::ImportedConfigModel;
use crate::terminal::model::TerminalModel;
use crate::workspace::view::tests::{initialize_app_for_move_session, mock_workspace};

fn workspace_for_palette(app: &mut App) -> ViewHandle<Workspace> {
    initialize_app_for_move_session(app);
    app.add_singleton_model(ImportedConfigModel::new);
    app.add_singleton_model(|_| SelectedItems::new());
    let workspace = mock_workspace(app);
    // Mock windows have no rendered responder chain for the terminal's editor.
    app.update(|ctx| {
        let window_id = workspace.window_id(ctx);
        let editor_id = ctx.focused_view_id(window_id).unwrap();
        if editor_id != workspace.id() {
            ctx.record_view_parent(window_id, editor_id, workspace.id());
        }
    });
    workspace
}

#[test]
fn title_bar_search_matches_an_open_terminal_session() {
    let _files = FeatureFlag::CommandPaletteFileSearch.override_enabled(false);
    App::test((), |mut app| async move {
        let workspace = workspace_for_palette(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let terminal = workspace.focused_terminal_view(ctx).unwrap();
            terminal.update(ctx, |terminal, _| {
                let mut model = terminal.model.lock();
                // The workspace's mock PTY does not complete the shell handshake.
                *model = TerminalModel::mock(None, None);
                model.simulate_block("echo palette-session-sentinel", "palette-session-sentinel");
            });
            workspace.handle_action(
                &WorkspaceAction::OpenPalette {
                    mode: PaletteMode::Command,
                    source: PaletteSource::TitleBarSearchBar,
                    query: None,
                },
                ctx,
            );
        });
        let palette = workspace.read(&app, |workspace, _| workspace.palette.clone());
        palette.update(&mut app, |palette, ctx| {
            palette.search_bar.update(ctx, |search_bar, ctx| {
                search_bar.set_query("palette-session-sentinel".to_string(), ctx)
            });
        });

        palette.read(&app, |palette, ctx| {
            let result = palette
                .search_results(ctx)
                .next()
                .expect("title-bar search must find terminal sessions without a filter prefix");
            assert!(matches!(
                result.accept_result(),
                CommandPaletteItemAction::NavigateToSession { .. }
            ));
            assert!(
                result
                    .accessibility_label()
                    .contains("palette-session-sentinel")
            );
            assert_eq!(palette.active_query_filter(ctx), None);
        });
    });
}

#[test]
fn title_bar_search_still_finds_commands() {
    let _files = FeatureFlag::CommandPaletteFileSearch.override_enabled(false);
    App::test((), |mut app| async move {
        let workspace = workspace_for_palette(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(
                &WorkspaceAction::OpenPalette {
                    mode: PaletteMode::Command,
                    source: PaletteSource::TitleBarSearchBar,
                    query: None,
                },
                ctx,
            );
        });
        let palette = workspace.read(&app, |workspace, _| workspace.palette.clone());
        palette.update(&mut app, |palette, ctx| {
            palette.search_bar.update(ctx, |search_bar, ctx| {
                search_bar.set_query("Open Theme Picker".to_string(), ctx)
            });
        });

        palette.read(&app, |palette, ctx| {
            let result = palette
                .search_results(ctx)
                .next()
                .expect("title-bar search must still find commands");
            let CommandPaletteItemAction::AcceptBinding { binding } = result.accept_result() else {
                panic!("theme search should return a command");
            };
            assert_eq!(binding.name, "workspace:show_theme_chooser");
        });
    });
}

#[test]
fn title_bar_search_clears_the_previous_command_filter() {
    let _files = FeatureFlag::CommandPaletteFileSearch.override_enabled(false);
    App::test((), |mut app| async move {
        let workspace = workspace_for_palette(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(
                &WorkspaceAction::OpenPalette {
                    mode: PaletteMode::Command,
                    source: PaletteSource::Keybinding,
                    query: None,
                },
                ctx,
            );
        });
        workspace.read(&app, |workspace, ctx| {
            assert_eq!(
                workspace.palette.as_ref(ctx).active_query_filter(ctx),
                Some(QueryFilter::Actions)
            );
        });
        workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_action(
                &WorkspaceAction::OpenPalette {
                    mode: PaletteMode::Command,
                    source: PaletteSource::TitleBarSearchBar,
                    query: None,
                },
                ctx,
            );
        });
        workspace.read(&app, |workspace, ctx| {
            assert_eq!(workspace.palette.as_ref(ctx).active_query_filter(ctx), None);
        });
    });
}
