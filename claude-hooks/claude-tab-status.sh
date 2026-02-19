#!/bin/bash
# Pipe Claude activity status to zjstatus tab names
# {spin} placeholder is animated by zjstatus Timer

[[ -z "$ZELLIJ" ]] && exit 0

INPUT=$(cat)
EVENT=$(echo "$INPUT" | jq -r '.hook_event_name // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
PROJECT=$(basename "$CWD" 2>/dev/null)
[[ -z "$PROJECT" ]] && PROJECT="claude"

case "$EVENT" in
    SessionStart)
        SYMBOL="●"; COLOR="#ffdc00"
        ;;
    PreToolUse|UserPromptSubmit)
        SYMBOL="{spin}"; COLOR="#ffdc00"
        ;;
    PostToolUse|SubagentStop)
        SYMBOL="{spin}"; COLOR="#ffdc00"
        ;;
    PermissionRequest)
        SYMBOL="⚠"; COLOR="#ff4136"
        ;;
    Notification)
        SYMBOL="!"; COLOR="#ff4136"
        ;;
    Stop)
        # Don't persist state - SessionEnd will clean up
        zellij pipe --name title --args "pane_id=${ZELLIJ_PANE_ID},fallback=${PROJECT},session=${ZELLIJ_SESSION_NAME}" -- "🤖 #[fg=#2ecc40]✓#[fg=]" 2>/dev/null
        zellij pipe -- "zjstatus::notify::${PROJECT} ✓ done" 2>/dev/null
        exit 0
        ;;
    SessionEnd)
        zellij pipe --name title --args "pane_id=${ZELLIJ_PANE_ID},session=${ZELLIJ_SESSION_NAME}" -- "" 2>/dev/null
        exit 0
        ;;
    *)
        exit 0
        ;;
esac

TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

PAYLOAD="🤖 #[fg=${COLOR}]${SYMBOL}#[fg=]"

# Broadcast to all plugins (zjstatus picks up name="title", others ignore it)
# When new tabs are created, existing zjstatus instances re-broadcast their state
zellij pipe --name title --args "pane_id=${ZELLIJ_PANE_ID},fallback=${PROJECT},session=${ZELLIJ_SESSION_NAME}" -- "$PAYLOAD" 2>/dev/null &

# zjstatus notification + desktop notification for important events
NOTIF_EXEC="zellij pipe --name focus --args pane_id=${ZELLIJ_PANE_ID},session=${ZELLIJ_SESSION_NAME}"

notify() {
    zellij pipe -- "zjstatus::notify::$1" 2>/dev/null &
    terminal-notifier -title "Claude Code" -message "$1" \
        -activate com.mitchellh.ghostty \
        -execute "$NOTIF_EXEC" &
}

case "$EVENT" in
    PermissionRequest)
        notify "${PROJECT} ⚠ needs permission"
        ;;
    Notification)
        notify "${PROJECT} ! waiting for input"
        ;;
esac
[[ "$EVENT" == "PreToolUse" && "$TOOL" == "AskUserQuestion" ]] && {
    notify "${PROJECT} ? asking"
}

exit 0
