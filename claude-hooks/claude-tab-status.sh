#!/usr/bin/env bash
set -euo pipefail

# This hook is only useful inside a Zellij session.
if [[ -z "${ZELLIJ:-}" ]]; then
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

if ! command -v zellij >/dev/null 2>&1; then
  exit 0
fi

input="$(cat || true)"
event="$(printf "%s" "$input" | jq -r '.hook_event_name // empty' 2>/dev/null)"
cwd="$(printf "%s" "$input" | jq -r '.cwd // empty' 2>/dev/null)"
project="$(basename "$cwd" 2>/dev/null || true)"

if [[ -z "$project" ]]; then
  project="claude"
fi

case "$event" in
  SessionStart)
    payload="AI #[fg=#ffdc00]active#[fg=]"
    ;;
  PreToolUse|PostToolUse|UserPromptSubmit|SubagentStop)
    payload="AI #[fg=#ffdc00]{spin}#[fg=]"
    ;;
  PermissionRequest)
    payload="AI #[fg=#ff4136]perm#[fg=]"
    ;;
  Notification)
    payload="AI #[fg=#ff4136]wait#[fg=]"
    ;;
  Stop)
    zellij pipe --name title \
      --args "pane_id=${ZELLIJ_PANE_ID:-},fallback=${project},session=${ZELLIJ_SESSION_NAME:-}" \
      -- "AI #[fg=#2ecc40]ok#[fg=]" >/dev/null 2>&1 || true
    zellij pipe -- "zjstatus::notify::${project} ok done" >/dev/null 2>&1 || true
    exit 0
    ;;
  SessionEnd)
    zellij pipe --name title \
      --args "pane_id=${ZELLIJ_PANE_ID:-},session=${ZELLIJ_SESSION_NAME:-}" \
      -- "" >/dev/null 2>&1 || true
    exit 0
    ;;
  *)
    exit 0
    ;;
esac

zellij pipe --name title \
  --args "pane_id=${ZELLIJ_PANE_ID:-},fallback=${project},session=${ZELLIJ_SESSION_NAME:-}" \
  -- "$payload" >/dev/null 2>&1 &

focus_cmd="zellij pipe --name focus --args pane_id=${ZELLIJ_PANE_ID:-},session=${ZELLIJ_SESSION_NAME:-}"

notify() {
  local message="$1"
  zellij pipe -- "zjstatus::notify::${message}" >/dev/null 2>&1 || true
  if command -v terminal-notifier >/dev/null 2>&1; then
    terminal-notifier \
      -title "Claude Code" \
      -message "$message" \
      -execute "$focus_cmd" >/dev/null 2>&1 || true
  fi
}

case "$event" in
  PermissionRequest)
    notify "${project} needs permission"
    ;;
  Notification)
    notify "${project} waiting for input"
    ;;
esac

tool_name="$(printf "%s" "$input" | jq -r '.tool_name // empty' 2>/dev/null)"
if [[ "$event" == "PreToolUse" && "$tool_name" == "AskUserQuestion" ]]; then
  notify "${project} asking a question"
fi

exit 0
