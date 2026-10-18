# Research: WarpSpeed Session Performance and Reliability

## Decision 1: Bound output before compact encoding

**Decision**: Enforce decoded budgets at block creation, legacy deserialization, and window checkpoint assembly. Use a 1 MiB block-output budget, 8 MiB pane budget, 16 MiB window budget, 2,048 restored blocks per pane, 4,096 per window, 256 pane histories, and a 512 MiB pre-deserialization file limit. Preserve block metadata within retained history and preferentially retain newer blocks and output.

**Rationale**: The observed 142.7 MB checkpoint contains 46.2 MB of actual styled bytes represented as JSON integer arrays. Compression or base64 alone reduces disk size but does not bound allocation, replay, or terminal parsing.

**Alternatives considered**:

- Compression only: rejected because decoded memory and replay remain unbounded.
- Lowering only the 5,000-line limit: rejected because styled bytes per line vary by orders of magnitude.
- Dropping all block output: rejected because useful recent recovery history would be lost.
- Leaving block-list cardinality unbounded: rejected because metadata-only histories could still create unbounded restoration work.

## Decision 2: Use a compact, backward-readable V2 JSON checkpoint

**Decision**: Keep checkpoint metadata in JSON, encode `serde_bytes` fields as base64 for V2, and accept both legacy integer arrays and V2 strings while reading. Validate version 1 and version 2, and rewrite version 1 through the existing atomic checkpoint path after budgets are applied.

**Rationale**: Base64 limits the encoded expansion to about 4/3 rather than about 3 times for decimal arrays. A 16 MiB decoded window budget remains below the 25 MiB on-disk target without introducing an additional compressed-file lifecycle or changing catalog paths.

**Alternatives considered**:

- JSON plus zstd: deferred because base64 plus decoded budgets meets the target with fewer recovery paths.
- Binary sidecars: rejected for this fix because coordinating manifest and sidecar atomicity adds failure states.
- Global `SerializedBlock` wire-format replacement: rejected because that type is used by other persistence and sharing paths.

## Decision 3: Migrate opportunistically with rollback copies

**Decision**: Read V1 through a bounded byte visitor, retain a `.v1.bak` copy before the first V2 replacement, write V2 to a temporary file, sync, parse-validate it, and atomically replace the active path. Never remove the backup in the initial release.

**Rationale**: Existing oversized files must be recoverable without manual deletion, and rollback to an older build needs the old representation. Bounded deserialization avoids allocating the complete legacy byte arrays.

**Alternatives considered**:

- In-place conversion: rejected because interruption could destroy the only checkpoint.
- Loading normally and trimming afterward: rejected because it incurs the current peak memory and parse cost.
- Automatic deletion of old checkpoints: rejected as destructive and incompatible with rollback.

## Decision 4: Treat restoration as a lifecycle batch

**Decision**: Suppress prompt refresh for `BlockType::Restored` and perform one refresh after replay creates the final active block and a live shell session is available. Missing execution context is a waiting state, not an error.

**Rationale**: Every restored block currently causes prompt work even though no executable session is available, producing repeated warnings and retries. The final prompt is the only state users observe.

**Alternatives considered**:

- Change warning to debug: rejected because command preparation and state churn still occur.
- Cache the error fingerprint only: insufficient because per-block refresh and other restored-command side effects remain.

## Decision 5: Represent repository knowledge explicitly

**Decision**: Distinguish unknown repository state, known non-repository state, and an attached repository model. Skip Git commands for known non-repositories. For unknown remote state, use quiet fallback commands and cache deterministic failures by session, working directory, and invalidating-command fingerprint.

**Rationale**: `None` currently means both "not in a repository" and "no native model available," causing unnecessary shell fallback. Explicit state removes ambiguity without disabling remote support.

**Alternatives considered**:

- Always run `git rev-parse` first: rejected for known local non-repositories because it still spawns a process.
- Permanently suppress after one failure: rejected because `cd` and Git commands can change applicability.

## Decision 6: Use bounded per-process prompt diagnostic files

**Decision**: Give each process a PID-qualified prompt diagnostic file, a bounded `sync_channel`, 4 KiB stdout and 4 KiB stderr excerpts, 10 MiB file rotation, and global retention of three recent files. Include original lengths and truncation flags. Do not forward payloads to ordinary logs.

**Rationale**: Separate files eliminate truncate-versus-stale-offset holes. Bounded records, queues, and retention prevent diagnostics from becoming a performance or disk-space issue.

**Alternatives considered**:

- Shared append-only file: rejected because cross-process record atomicity and rotation remain platform-sensitive.
- OS file locking: rejected because it adds platform-specific coordination solely for debug diagnostics.
- Disable diagnostics: rejected because dogfood investigation still needs them.

## Decision 7: Reuse platform single-instance services for WarpSpeed

**Decision**: Decouple existing Windows and Linux single-instance forwarding from `release_bundle` and enable it for WarpSpeed GUI app mode. Key mutex, named pipe, advisory lock, and D-Bus identity by the effective AppId/profile while retaining existing release-channel identities. Linux claims the profile-local advisory lock before profile state opens and uses D-Bus for forwarding. CLI and helper roles bypass GUI ownership. Full-GUI window-process replacements do not bypass ownership, and every non-empty sanitized profile retains a hashed identity.

**Rationale**: The repository already has startup forwarding and lifecycle handling. WarpSpeed currently omits it only because its standalone binary does not enable `release_bundle`.

**Alternatives considered**:

- Depend only on the asynchronously started Linux D-Bus service for ownership: rejected because simultaneous launches can both open profile state before one wins the D-Bus name.
- Lock by executable version: rejected because it permits old and new versions to write one profile concurrently.
- Lock all profiles together: rejected because `WARPSPEED_PROFILE` intentionally isolates data.
- Treating a window-process replacement as a helper: rejected because it initializes the same profile as a full GUI writer.

## Decision 8: Verify process and filesystem behavior cross-platform

**Decision**: Run focused local Rust tests first, then verify Windows named-mutex/rename behavior, Linux D-Bus/rename behavior, and macOS checkpoint compatibility. Architecture fan-out is unnecessary because no ABI, unsafe layout, or architecture-specific serialization is introduced.

**Rationale**: The changes are operating-system-sensitive but not architecture-sensitive.

**Alternatives considered**:

- Windows-only verification: rejected because checkpoint read/write code is shared and Linux has its own startup forwarding path.
