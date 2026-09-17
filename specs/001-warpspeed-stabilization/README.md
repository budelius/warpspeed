# WarpSpeed Work Queue

Start with [plan.md](plan.md) for the branch inventory and execution order, then
[tasks.md](tasks.md) for the actionable checklist. [spec.md](spec.md) defines acceptance criteria.

Spec Kit 1.0.6 is initialized using its generic integration. Workflow prompts live under
`.specify/commands/`; they are files to use with your coding agent, not automatically registered
Pi slash commands. No agent-specific integration is implied.

To validate these documents without changing the current feature branch:

```sh
SPECIFY_FEATURE=001-warpspeed-stabilization \
SPECIFY_FEATURE_DIRECTORY="$PWD/specs/001-warpspeed-stabilization" \
  bash .specify/scripts/bash/check-prerequisites.sh \
  --json --require-spec --require-tasks --include-tasks
```

This prerequisite check passed when the plan was created. It checks document presence, not Rust
correctness or completion of feature acceptance criteria. `git diff --check` also passed.

The immediate next coding task is T005–T009 on `fix/macos-local-build`; the uncommitted change to
`app/src/bin/oss.rs` must remain independent until its intended behavior is confirmed.

Operational update: all three TM clone retries still failed DNS resolution. No repository was
removed or overwritten. CEFA and really-old-stuff still require confirmed remotes.
