# WarpSpeed Profile Ownership Contract

## Scope

Ownership applies only to GUI application launches. CLI subcommands, crash helpers, and terminal supervisors do not compete for GUI ownership. WarpSpeed rejects the existing window-process handoff path because its replacement is a full GUI writer; users restart the profile owner instead.

## Identity

The ownership key is derived from the effective AppId/profile and user session. It is independent of executable path and application version. Explicit profiles and elevation-derived profiles therefore remain isolated. Every non-empty explicit profile includes a hash of its exact value after sanitization, including names made entirely of unsupported characters, so sanitization and case-folding cannot collapse distinct profiles into the same identity or the default profile.

## Launch behavior

| Existing owner | New launch | Required result |
|---|---|---|
| None | GUI | New process becomes primary and starts the profile IPC service. |
| Same profile and version | GUI | URLs or new-window intent are forwarded; new process exits. |
| Same profile, different version | GUI | Intent is forwarded with version metadata; no second writer starts; restart-required state is surfaced. |
| Different explicit profile | GUI | New process may become that profile's primary. |
| Any | CLI/helper/supervisor | Process performs its role without acquiring GUI ownership. |

## Failure behavior

- Kernel or desktop-service ownership is released automatically on primary-process death.
- IPC startup failure cannot leave a process claiming ownership while unable to receive launch intents.
- If forwarding fails unexpectedly, the application reports the failure but does not start a concurrent writer for an ownership key known to be held.
- Live terminal supervisors remain running during GUI replacement and may be adopted by the new primary through existing supervisor protocols.

## Platform mapping

- Windows: named mutex plus named-pipe URI service, names scoped by effective profile.
- Linux/FreeBSD: profile-local advisory ownership lock plus profile-specific D-Bus well-known name and object path.
- macOS: existing application-instance behavior plus shared checkpoint compatibility; no new architecture-specific contract.
