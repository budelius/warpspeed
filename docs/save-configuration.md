# Saving a window configuration

Open **Save or update session or launch configuration** from the Command Palette.

- The target picker defaults to the window's original configuration, including TOML sessions.
- Search the picker to choose an existing **Session** or **Launch** configuration. Launch targets
  identify the YAML document and window, so updating one does not replace other windows.
- **Save as new launch configuration (YAML)** creates a separate file. A name collision never
  silently replaces an entire file: select its existing target or choose another name.
- Selecting a target does not write anything. **Update Configuration** performs the update.

Session updates retain the session ID, name, generation, and existing ownership. The current layout
and working directories are captured into private tab-template copies before atomically replacing
that session's TOML manifest. Previously referenced templates are retained, not edited or deleted,
so other sessions sharing them are unaffected. Startup commands, template parameters, and shell
overrides are not copied from running shells. Shell/agent recovery metadata remains in automatic
checkpoints according to the Restore session setting, not in these exported templates.

Saving into another configuration does not attach this window to that session, acquire its lease,
restart its shells, or replace its live/recovery state. Existing sessions still resume their saved
checkpoints; these templates describe fresh starts. Save-as-new associates an otherwise unnamed
window only with the newly created YAML, never with an existing target owned elsewhere.

## Verification

Unit/model tests cover current-session preselection, distinguishing same-named formats and multiple
YAML windows, switching targets, reopen/reset behavior, ownership preservation, session-file
round trips, shared-template preservation, invalid identities, stale generations, path containment,
concurrent writes, and failed-write cleanup. Filesystem tests use temporary directories.

`test_save_configuration_target_picker` exercises the Command Palette, searchable picker, and TOML
save. Both save-dialog GUI tests use fresh authored-config directories beneath the test root;
Windows AppData does not follow the harness's temporary HOME. They must not run against a production
WarpSpeed profile: authored configuration directories are shared across `WarpSpeed-*` profiles.

The current integration runner rejects Windows execution before app startup because hermetic
platform support is not implemented there. Compilation and unit tests do not replace GUI acceptance
on a supported host.
