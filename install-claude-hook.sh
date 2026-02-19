#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required to install the Claude hook." >&2
  exit 1
fi

CLAUDE_DIR="${HOME}/.claude"
HOOKS_DIR="${CLAUDE_DIR}/hooks"
SETTINGS="${CLAUDE_SETTINGS:-${CLAUDE_DIR}/settings.json}"
HOOK_NAME="claude-tab-status.sh"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK_SRC="${SCRIPT_DIR}/claude-hooks/${HOOK_NAME}"
HOOK_DST="${HOOKS_DIR}/${HOOK_NAME}"

# Events used by the zjstatus Claude hook.
EVENTS=(
  SessionStart
  SessionEnd
  Stop
  SubagentStop
  Notification
  UserPromptSubmit
  PermissionRequest
  PreToolUse
  PostToolUse
)

if [[ ! -f "$HOOK_SRC" ]]; then
  echo "error: missing hook source: $HOOK_SRC" >&2
  exit 1
fi

echo "Installing zjstatus Claude hook..."

mkdir -p "$HOOKS_DIR"
cp "$HOOK_SRC" "$HOOK_DST"
chmod +x "$HOOK_DST"
echo "  copied: $HOOK_DST"

if [[ ! -f "$SETTINGS" ]]; then
  mkdir -p "$(dirname "$SETTINGS")"
  printf '{"hooks":{}}\n' > "$SETTINGS"
fi

if ! jq empty "$SETTINGS" >/dev/null 2>&1; then
  echo "error: invalid JSON in $SETTINGS" >&2
  exit 1
fi

for event in "${EVENTS[@]}"; do
  already_registered="$(jq -r \
    --arg evt "$event" \
    --arg cmd "$HOOK_DST" \
    '(.hooks[$evt] // []) | [ .[].hooks[]? | select(.type == "command" and .command == $cmd) ] | length' \
    "$SETTINGS")"

  if [[ "$already_registered" != "0" ]]; then
    echo "  ${event}: already registered"
    continue
  fi

  has_catchall="$(jq -r \
    --arg evt "$event" \
    '(.hooks[$evt] // []) | [ .[] | select((.matcher // "") == "") ] | length' \
    "$SETTINGS")"

  if [[ "$has_catchall" != "0" ]]; then
    jq --arg evt "$event" \
      --arg cmd "$HOOK_DST" \
      '
      .hooks = (.hooks // {}) |
      .hooks[$evt] |= [
        .[] |
        if ((.matcher // "") == "") then
          .hooks = ((.hooks // []) + [{"type":"command","command":$cmd}])
        else
          .
        end
      ]
      ' "$SETTINGS" > "${SETTINGS}.tmp"
  else
    jq --arg evt "$event" \
      --arg cmd "$HOOK_DST" \
      '
      .hooks = (.hooks // {}) |
      .hooks[$evt] = ((.hooks[$evt] // []) + [
        {"matcher":"","hooks":[{"type":"command","command":$cmd}]}
      ])
      ' "$SETTINGS" > "${SETTINGS}.tmp"
  fi

  mv "${SETTINGS}.tmp" "$SETTINGS"
  echo "  ${event}: registered"
done

echo "Done."
echo "Hook installed at: $HOOK_DST"
echo "Settings updated:  $SETTINGS"
