# Cause found

Warp’s main UI thread is stuck in a render/update loop:

- UI thread: ~71% of one CPU core
- GPU: ~16% average, peaking near 19%
- Warp memory: 2.26 GB working / 5.04 GB private
- Runtime: 7.5 days
- Restored workspace: 43 terminal panes, previously a 161-process tree

The trigger is Warp’s Kubernetes Context prompt chip. It fans out:

```text
kubectl config current-context
```

across restored panes—up to 87 executions in one second. It had also abandoned 33 suspended kubectl processes.

I removed only those inert probes, recovering roughly 484 MB, 56 threads, and 170 handles. No shells or agent sessions were touched. However, the UI thread remains hot, so a Warp restart is required for full recovery.

Also, the visible Warp window is still running build 20261019...; launches are being forwarded to it instead of using the current 20261021... build.

## Recommended

1. Disable/remove the Kubernetes Context prompt chip.
2. Gracefully restart the old Warp window.
3. Reduce unused restored panes; Warp’s local state is already 4.2 GB.

I have not restarted Warp because it currently owns multiple active agent sessions.
