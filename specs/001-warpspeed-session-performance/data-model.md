# Data Model: WarpSpeed Session Performance and Reliability

## SessionCheckpointV2

A versioned window recovery document.

| Field | Type | Rules |
|---|---|---|
| `version` | integer | Must be `2` when written; readers also accept `1` for migration. |
| `source` | launch-config source | Identifies the session generation and window. |
| `profile` | optional string | Effective application profile that owns the checkpoint. |
| `window` | window snapshot | Restorable layout and pane metadata; recoverable independently of block output. |
| `block_lists` | list of bounded pane histories | Total decoded output and retained history cardinality must not exceed the window budgets. |
| `saved_at_unix_seconds` | integer | Used for catalog ordering and migration diagnostics. |

### Validation

- A checkpoint larger than 512 MiB is rejected before deserialization.
- Version must be supported before the checkpoint is selected for restoration.
- Malformed V2 output fields restore with a bounded unavailable-output marker while preserving block metadata; malformed command fields invalidate the checkpoint.
- Decoded bytes beyond a declared budget are discarded while counting the original size.
- A replacement checkpoint must deserialize successfully before it becomes active.

## BoundedPaneHistory

| Field | Type | Rules |
|---|---|---|
| `uuid` | bytes | Must match a terminal pane in the window snapshot. |
| `blocks` | ordered list | Commands remain chronological; at most 2,048 recent blocks are retained per pane and 4,096 per window. |
| `retained_output_bytes` | derived integer | Must not exceed the pane budget. |

At most 256 pane histories are retained. When a cardinality or output budget is exceeded, older history is removed or shortened before newer history.

## PersistedBlockOutput

| Field | Type | Rules |
|---|---|---|
| `stylized_output` | bytes | Must not exceed the per-block budget. |
| `original_output_bytes` | optional integer | Populated when known and output was truncated. |
| `output_truncated` | boolean | Defaults to false for legacy data. |

### State transitions

```text
Complete -> Bounded
Complete -> Truncated
Legacy oversized -> Truncated during decode
Malformed -> Output omitted, block metadata retained
```

The retained sequence starts from a reset state and ends by cancelling any incomplete control sequence before resetting terminal styles. A truncation marker is inserted without carrying user content into diagnostics.

## PromptRefreshState

| State | Meaning |
|---|---|
| `Unavailable` | The chip does not apply to this surface or runtime. |
| `WaitingForContext` | A live session or shell context is not available yet. |
| `Loading` | One bounded asynchronous generator is running. |
| `Cached` | The current fingerprint already has a usable value or suppressed deterministic failure. |
| `Ready` | A current value is available. |
| `Disabled` | Runtime capability or known context says the chip does not apply. |
| `TimedOut` | A command exceeded its deadline and may retry after a relevant change. |
| `Error` | An unexpected execution failure occurred after valid preparation. |

`WaitingForContext` transitions only on session/context lifecycle events. It does not schedule a polling loop.

## RepositoryContext

| Variant | Meaning | Git fallback allowed? |
|---|---|---|
| `Unknown` | Native determination is unavailable or pending. | Only for supported remote sessions. |
| `NotRepository` | The current local path is known not to belong to a repository. | No. |
| `Attached` | A repository status source is available. | No; external metadata is authoritative. |

Transitions are driven by working-directory changes and repository-model subscription changes.

## PromptDiagnosticRecord

| Field | Type | Rules |
|---|---|---|
| timestamp | time | Millisecond precision. |
| chip kind/title/phase | metadata | Must not contain terminal output. |
| shell type | enum | Diagnostic metadata. |
| working directory | optional local-only text | Written only to dogfood debug diagnostics. |
| command | local-only text | Bounded; never copied to ordinary logs. |
| stdout/stderr excerpt | bytes rendered lossily | Each limited to 4 KiB. |
| stdout/stderr original length | integer | Recorded when an excerpt is shorter than the captured stream. |
| truncation marker | text metadata | Includes retained and original byte counts when truncated. |
| status/timeout/exit code | metadata | Content-free. |

Records enter a bounded queue. A full queue drops the new record and increments a content-free counter.

## ProfileOwnership

| Field | Type | Rules |
|---|---|---|
| profile identity | stable string | Derived from effective AppId, including explicit profile/elevation isolation. |
| process role | enum | GUI primary, CLI, crash helper, or terminal supervisor. |
| process id | integer | Diagnostic only; kernel ownership remains authoritative. |
| application version | string | Used to report version mismatch. |
| IPC endpoint | platform address | Derived from profile identity, not executable version. |

### State transitions

```text
Unowned -> PrimaryOwned
PrimaryOwned + same-version GUI launch -> Forwarded
PrimaryOwned + different-version GUI launch -> Forwarded with restart-required metadata
Primary exits/crashes -> Unowned
Unowned + waiting GUI -> PrimaryOwned
```

Helper and supervisor processes never enter `PrimaryOwned`. A full-GUI window-process replacement is not a helper and is rejected for WarpSpeed rather than bypassing ownership.

## MigrationResult

| Field | Type | Rules |
|---|---|---|
| source version | integer | `1` for the initial migration. |
| destination version | integer | `2`. |
| source bytes | integer | Content-free measurement. |
| destination bytes | integer | Content-free measurement. |
| truncated block count | integer | Content-free measurement. |
| backup path | local path | Uses `.v1.bak`; logged only through safe local diagnostics. |
| validation status | enum | Pending, validated, committed, or failed. |

A failed migration cannot delete or replace the source checkpoint.
