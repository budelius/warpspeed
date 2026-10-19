# Session Checkpoint V2 Contract

## Compatibility

- Writers emit version `2`.
- Readers accept versions `1` and `2`.
- Version 1 byte arrays may be JSON integer sequences.
- Version 2 `serde_bytes` payloads are base64 JSON strings.
- Unsupported versions fail without modifying the file.

## Resource limits

Default decoded limits:

| Scope | Limit |
|---|---:|
| One command | 64 KiB |
| One block output | 1 MiB |
| One pane's block output | 8 MiB |
| One window's block output | 16 MiB |
| Restored blocks per pane | 2,048 |
| Restored blocks per window | 4,096 |
| Pane block lists per window | 256 |
| Input checkpoint file | 512 MiB |

Byte limits apply to decoded output except for the explicit input-file limit. Encoded size and compression ratio never increase decoded-output limits. Files exceeding the input limit are rejected before deserialization; cardinality limits retain the newest histories.

## Truncation

- Block metadata is retained when output is truncated.
- Newer blocks have priority over older block output.
- Retained styled output begins with a terminal reset and ends with a control-sequence cancellation followed by a style reset.
- Restored history visibly indicates omitted output.
- Truncation metadata records that omission occurred and the original size when available.

## Migration transaction

1. Open the V1 source without replacing it.
2. Decode byte arrays through bounded visitors.
3. Assemble a budget-compliant V2 checkpoint.
4. Create the rollback copy without overwriting an existing valid backup.
5. Write V2 to a temporary file in the same directory.
6. Flush and sync the temporary file.
7. Deserialize and validate the temporary V2 file.
8. Atomically replace the active checkpoint.
9. Keep the V1 rollback copy.

Failure before step 8 leaves the original active. Failure at step 8 leaves either the original or replacement active according to platform atomic-replace guarantees, with the rollback copy retained.

## Recovery

If a base64 payload or legacy integer-array output is invalid but the outer document and layout are readable, restoration substitutes a bounded unavailable-output marker and retains block metadata. Malformed command bytes invalidate the checkpoint. A malformed outer document is skipped and its rollback copy is considered before catalog removal or rebuild.
