# Runtime Model

## Terms

### Control Workspace

The directory where the user launches picoagent.

It owns:
- `config.md`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `memory/`
- `skills/`
- `agents/`

This is the source of prompt framing and user intent.

### Execution Repo

The filesystem root where the main agent executes commands and where worker task workspaces are derived from.

Behavior:
- if the control workspace is inside a git repository, picoagent attaches to that repository
- otherwise picoagent creates an isolated git snapshot under the run workspace

This gives workers a real repository to branch from without forcing the docs/prompt source to move.

### Task Workspace

One directory per dispatched task.

Behavior:
- when a git repo is available, tasks are created as git worktrees
- otherwise tasks fall back to plain directories

Workers run with:
- `cwd = task workspace`
- `writeRoot = task workspace`

That is the core write boundary.

## Why This Split Exists

The project needs two things at once:

- stable prompt sources from the user-managed workspace
- isolated execution surfaces for background workers

Those are different concerns. Treating them as the same directory caused ambiguous behavior and stale docs.

## Prompt Loading

Main prompt assembly reads from the control workspace:

- `SOUL.md`
- `USER.md`
- `AGENTS.md`
- `memory/memory.md`
- skill summaries
- agent summaries

Worker prompt assembly reads framing from the control workspace but executes inside the task workspace.

This means:
- user/project intent stays stable
- task execution can be isolated

## Task Lifecycle

Each task directory contains:
- `task.md`
- `progress.md`
- `result.md`

Status flow:
- `pending`
- `running`
- `completed`
- `failed`
- `aborted`

## Sandbox Boundary

For worker commands:
- the shell runs in the task workspace
- writes are restricted to the task workspace
- the sandbox may fall back to plain shell execution if `bwrap` is unavailable, but the logical write boundary still applies through `write_file`

## Design Consequences

- Prompt files are not copied around just to make the runtime work.
- Runtime assembly must explicitly carry both control-root and execution-root context.
- Entrypoints should log which repo mode they are using so the operator can see whether picoagent attached to a real git repo or created an isolated snapshot.
