# zjstatus fork - Tab name overrides & Claude Code integration

This fork adds dynamic tab naming based on pane activity, designed for Claude Code integration.

## Features

### Tab name overrides via pipe

Panes can override their tab's display name by sending a `title` pipe:

```bash
zellij pipe --name title \
  --args "pane_id=${ZELLIJ_PANE_ID},fallback=${PROJECT},session=${ZELLIJ_SESSION_NAME}" \
  -- "🤖 #[fg=#ffdc00]{spin}#[fg=]"
```

- `pane_id`: The pane sending the override (required)
- `session`: Target session (filters out other sessions)
- `fallback`: Project name shown alongside the status
- Payload: The status text (supports `#[fg=...]` color codes)
- `{spin}` placeholder: Animated spinner (✦ ✶ ✽ ✶)

Empty payload clears the override:
```bash
zellij pipe --name title --args "pane_id=${ZELLIJ_PANE_ID},session=${ZELLIJ_SESSION_NAME}" -- ""
```

### Multiple Claude instances per tab

When multiple panes have overrides on the same tab, symbols are chained:
```
🤖 ✦ ● (+1) | project-name
```

### Focus pane via pipe

Focus a pane from anywhere (useful for notification click-to-focus):

```bash
zellij pipe --name focus --args "pane_id=${ZELLIJ_PANE_ID},session=${ZELLIJ_SESSION_NAME}"
```

This switches to the correct tab AND focuses the pane.

### State persistence

Overrides are persisted to `/tmp/zjstatus-${session}.state` so new zjstatus instances can restore state on startup.

### Tab position stability

When tabs are closed/reordered, overrides are automatically remapped to the correct tab positions based on `pane_id` (which is stable, unlike tab positions).

## Claude Code hook example

See `~/.claude/hooks/claude-tab-status.sh` for a complete integration that:
- Shows activity status (spinning while working, checkmark when done)
- Displays project name from `cwd`
- Sends desktop notifications with click-to-focus
