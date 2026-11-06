use super::*;
use crate::app_state::{
    LeafContents, LeafSnapshot, PaneNodeSnapshot, TabSnapshot, TerminalPaneSnapshot,
};
use crate::tab::SelectedTabColor;

const ORIGINAL: &str = r#"# Keep this identity and comment.
version = 1
id = "workspace"
name = "Workspace"
generation = 7
active_tab = "original"
[[tabs]]
id = "original"
config = "shared.toml"
[tabs.params]
repo = "/old"
"#;
const SHARED: &str = r#"name = "Shared"
[[panes]]
id = "main"
type = "terminal"
directory = "/old"
commands = ["echo original-startup"]
"#;

struct Fixture {
    root: tempfile::TempDir,
    sessions: std::path::PathBuf,
    tabs: std::path::PathBuf,
    source: LaunchConfigWindowSource,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let tabs = root.path().join("tabs");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&tabs).unwrap();
        fs::write(tabs.join("shared.toml"), SHARED).unwrap();
        let path = sessions.join("workspace.toml");
        fs::write(&path, ORIGINAL).unwrap();
        Self {
            root,
            sessions,
            tabs,
            source: LaunchConfigWindowSource {
                path,
                document_index: 7,
                window_index: 0,
                config_name: "Workspace".into(),
            },
        }
    }

    fn save(&self, window: &WindowSnapshot) -> Result<LaunchConfig> {
        overwrite_window_in_dirs(&self.sessions, &self.tabs, &self.source, window)
    }
}

fn window() -> WindowSnapshot {
    WindowSnapshot {
        tabs: vec![TabSnapshot {
            custom_title: Some("New tab".into()),
            root: PaneNodeSnapshot::Leaf(LeafSnapshot {
                is_focused: true,
                custom_vertical_tabs_title: None,
                contents: LeafContents::Terminal(TerminalPaneSnapshot {
                    uuid: vec![1, 2, 3],
                    persistent_session_id: None,
                    session_holder: None,
                    cli_agent_resume_descriptor: None,
                    managed_session_job_id: None,
                    cwd: Some("/updated".into()),
                    shell_launch_data: None,
                    is_active: true,
                    is_read_only: false,
                    input_config: None,
                    llm_model_override: None,
                    active_profile_id: None,
                    conversation_ids_to_restore: vec![],
                    active_conversation_id: None,
                }),
            }),
            default_directory_color: None,
            selected_color: SelectedTabColor::Cleared,
            left_panel: None,
            right_panel: None,
            group_id: None,
            pinned: false,
        }],
        active_tab_index: 0,
        team_uid: None,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: None,
        warp_ai_width: None,
        voltron_width: None,
        warp_drive_index_width: None,
        left_panel_open: false,
        vertical_tabs_panel_open: false,
        left_panel_width: None,
        right_panel_width: None,
        agent_management_filters: None,
        launch_config_source: None,
        tab_groups: vec![],
    }
}

#[test]
fn saved_tab_references_use_portable_path_separators() {
    let fixture = Fixture::new();
    fixture.save(&window()).unwrap();
    let contents = fs::read_to_string(&fixture.source.path).unwrap();
    let stored: SessionConfigFile = toml::from_str(&contents).unwrap();
    let reference = stored.tabs[0].config.to_str().unwrap();
    assert!(reference.contains('/'));
    assert!(!reference.contains('\\'));
    assert!(fixture.tabs.join(reference).is_file());
}

#[test]
fn updates_session_layout_without_changing_its_identity_or_shared_templates() {
    let fixture = Fixture::new();
    let mut snapshot = window();
    snapshot.tabs.push(snapshot.tabs[0].clone());
    snapshot.tabs[1].custom_title = Some("Second".into());
    snapshot.active_tab_index = 1;
    let other = fixture.sessions.join("other.toml");
    let other_contents = ORIGINAL.replace("id = \"workspace\"", "id = \"other\"");
    fs::write(&other, &other_contents).unwrap();

    let updated = fixture.save(&snapshot).unwrap();

    assert_eq!(updated.window_source(0), Some(fixture.source.clone()));
    assert_eq!(updated.windows[0].active_tab_index, Some(1));
    assert_eq!(updated.windows[0].tabs.len(), 2);
    assert_eq!(updated.windows[0].tabs[1].title.as_deref(), Some("Second"));
    let contents = fs::read_to_string(&fixture.source.path).unwrap();
    assert!(contents.starts_with("# Keep this identity and comment."));
    let saved: SessionConfigFile = toml::from_str(&contents).unwrap();
    assert_eq!(saved.id, "workspace");
    assert_eq!(saved.generation, 7);
    assert_eq!(fs::read_to_string(&other).unwrap(), other_contents);
    assert_eq!(
        fs::read_to_string(fixture.tabs.join("shared.toml")).unwrap(),
        SHARED
    );
    assert!(!fixture.sessions.join("workspace.yaml").exists());
    let (loaded, errors) = super::super::load_session_configs(&fixture.sessions, &fixture.tabs);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(loaded.contains(&updated));
    assert_eq!(snapshot.launch_config_source, None);
}

#[test]
fn rejects_a_stale_session_generation_without_writing() {
    let mut fixture = Fixture::new();
    fixture.source.document_index = 6;
    assert!(
        fixture
            .save(&window())
            .unwrap_err()
            .to_string()
            .contains("generation")
    );
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
    assert_eq!(fs::read_dir(&fixture.tabs).unwrap().count(), 1);
}

#[test]
fn rejects_a_changed_session_name_without_writing() {
    let mut fixture = Fixture::new();
    fixture.source.config_name = "Renamed".into();
    assert!(fixture.save(&window()).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[test]
fn rejects_paths_outside_the_session_directory() {
    let mut fixture = Fixture::new();
    fixture.source.path = fixture.root.path().join("outside.toml");
    fs::write(&fixture.source.path, ORIGINAL).unwrap();
    assert!(fixture.save(&window()).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[test]
fn failed_tab_writes_leave_the_session_unchanged() {
    let mut fixture = Fixture::new();
    fixture.tabs = fixture.root.path().join("not-a-directory");
    fs::write(&fixture.tabs, "occupied").unwrap();
    assert!(fixture.save(&window()).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[test]
fn invalid_session_identity_cleans_up_unpublished_tabs() {
    let fixture = Fixture::new();
    let invalid = ORIGINAL.replace("id = \"workspace\"", "id = \"wrong-id\"");
    fs::write(&fixture.source.path, &invalid).unwrap();
    assert!(fixture.save(&window()).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), invalid);
    assert_eq!(fs::read_dir(&fixture.tabs).unwrap().count(), 1);
}

#[test]
fn concurrent_save_is_rejected_without_waiting_or_writing() {
    let fixture = Fixture::new();
    let lock = fs::File::create(fixture.source.path.with_extension("toml.save.lock")).unwrap();
    assert!(lock.try_lock_exclusive().unwrap());
    assert!(
        fixture
            .save(&window())
            .unwrap_err()
            .to_string()
            .contains("another process")
    );
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[test]
fn rejects_an_empty_window_without_overwriting_the_session() {
    let fixture = Fixture::new();
    let mut snapshot = window();
    snapshot.tabs.clear();
    assert!(fixture.save(&snapshot).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[test]
fn rejects_an_invalid_active_tab_without_overwriting_the_session() {
    let fixture = Fixture::new();
    let mut snapshot = window();
    snapshot.active_tab_index = 1;
    assert!(fixture.save(&snapshot).is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
}

#[cfg(windows)]
#[test]
fn failed_manifest_replacement_preserves_the_original_and_cleans_up_tabs() {
    let fixture = Fixture::new();
    let permissions = fs::metadata(&fixture.source.path).unwrap().permissions();
    let mut read_only = permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&fixture.source.path, read_only).unwrap();

    let result = fixture.save(&window());
    fs::set_permissions(&fixture.source.path, permissions).unwrap();

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), ORIGINAL);
    assert_eq!(fs::read_dir(&fixture.tabs).unwrap().count(), 1);
}
