#!/usr/bin/env bash
# Runs the server under the pinned MCP Inspector, in the niri session this shell is in.
#
#   scripts/inspector.sh web     opens the Inspector's web UI
#   scripts/inspector.sh check   lists the tools and calls each one through the Inspector's
#                                CLI, printing one pass or fail line per check
#
# The check prints names, keys and sizes only: never screenshot data, clipboard text or
# window titles. In both modes the audit log goes to a temporary directory that is
# removed on exit.
set -euo pipefail

readonly INSPECTOR=@modelcontextprotocol/inspector@2.9.0
# The Inspector starts the server with a minimal environment, so these are passed on: the
# same three a Codex user forwards. The server finds the session bus in XDG_RUNTIME_DIR.
readonly SESSION_VARS='["NIRI_SOCKET", "XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"]'

case "${1:-}" in
web | check) ;;
*)
  echo "usage: scripts/inspector.sh web | check" >&2
  exit 2
  ;;
esac

cargo build --locked -p niri-computer-use
server=$(realpath target/debug/niri-computer-use)
work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT
config="${work}/config.json"
jq -n --arg command "${server}" --arg state "${work}/state" --argjson names "${SESSION_VARS}" \
  '{mcpServers: {"niri-computer-use": {command: $command, args: ["serve"], env:
    ({XDG_STATE_HOME: $state} + ($ENV | with_entries(select(.key | IN($names[])))))}}}' \
  >"${config}"
# Keeps the Inspector's own settings out of your home directory.
export MCP_CLIENT_CONFIG_PATH="${work}/client.json"

cli=(npx -y "${INSPECTOR}" --cli --config "${config}" --server niri-computer-use --format json)

failures=0

# check <description> <jq filter that must be true> <file>
check() {
  if jq -e "$2" "$3" >/dev/null; then
    echo "ok   $1"
  else
    echo "FAIL $1"
    failures=$((failures + 1))
  fi
}

# call <name> <tool> [json arguments]: saves the result in $work/<name>.json. The
# Inspector exits 5 when a tool returns isError, which some checks expect.
call() {
  local status=0
  "${cli[@]}" --method tools/call --tool-name "$2" \
    --tool-args-json "${3:-"{}"}" >"${work}/$1.json" 2>"${work}/$1.err" || status=$?
  if [[ ${status} -ne 0 && ${status} -ne 5 ]]; then
    echo "FAIL $1: the Inspector exited with ${status}"
    failures=$((failures + 1))
  fi
}

# Every tool listed with niri reachable; Noctalia and the accessibility bus add the rest.
readonly TOOLS='["acquire_desktop","click","clipboard_read","close_window","desktop_state",
  "drag","focus_window","focus_workspace","key","launch","niri_action","outputs","paste","pointer_move",
  "release_desktop","screenshot","scroll","status","type_text","wait_for"]'
# Tools that only read; every other tool changes the desktop or the lease.
readonly READ_ONLY='["clipboard_read","desktop_state","elements","outputs","screenshot",
  "shell_status","status","wait_for"]'

# Runs after check_status, whose result says whether the accessibility bus was found.
list_tools() {
  local expected="${TOOLS}"
  if command -v noctalia >/dev/null; then
    expected="${expected} + [\"shell_close\",\"shell_open\",\"shell_status\"]"
  fi
  if jq -e '.result.structuredContent.accessibility.available' "${work}/status.json" >/dev/null; then
    expected="${expected} + [\"elements\"]"
  fi
  # --strict exits 6 on a schema portability error.
  local status=0
  "${cli[@]}" --method tools/list --strict >"${work}/tools.json" 2>"${work}/tools.err" ||
    status=$?
  if [[ ${status} -ne 0 ]]; then
    echo "FAIL tools/list: the Inspector exited with ${status}"
    failures=$((failures + 1))
  fi
  check "tools/list names the expected tools" \
    "[.result.tools[].name] | sort == (${expected} | sort)" "${work}/tools.json"
  # The check calls only read-only tools, so it never takes the lease from an agent using
  # this session or changes the desktop.
  check "exactly the observation tools are read-only" \
    "all(.result.tools[]; .annotations.readOnlyHint == (.name | IN(${READ_ONLY}[])))" \
    "${work}/tools.json"
  check "no schema portability findings" \
    '(.schemaFindings // []) | length == 0' "${work}/tools.json"
}

check_status() {
  call status status
  local instance
  instance=$(basename "${NIRI_SOCKET:-}")
  check "status: niri is supported and its event stream connected" \
    '.result.structuredContent.niri | .compat == "ok" and .event_stream == "connected"' \
    "${work}/status.json"
  check "status: the instance is NIRI_SOCKET's basename" \
    ".result.structuredContent.instance == \"${instance}\"" "${work}/status.json"
  # Holds in a graphical session started by logind, where the check is meant to run.
  check "status: the lock state comes from logind" \
    '.result.structuredContent.lock.source == "logind"' "${work}/status.json"
  check "status: the audit log is writable" \
    '.result.structuredContent.audit.last_error == null' "${work}/status.json"
}

check_observation() {
  call outputs outputs
  check "outputs: at least one enabled output" \
    '[.result.structuredContent[] | select(.logical != null)] | length > 0' \
    "${work}/outputs.json"
  call desktop_state desktop_state
  check "desktop_state: one snapshot with every field" \
    '.result.structuredContent | keys == ["focused_window","keyboard_layouts","overview_open","windows","workspaces"]' \
    "${work}/desktop_state.json"
  local focused
  focused=$(jq '.result.structuredContent.focused_window' "${work}/desktop_state.json")
  if [[ ${focused} != null ]] &&
    jq -e '.result.structuredContent.accessibility.available' "${work}/status.json" >/dev/null; then
    call elements elements "{\"window_id\": ${focused}, \"limit\": 5}"
    check "elements: the focused window's elements, or why it has none" \
      '.result.structuredContent | (.window_id != null and (.elements | type) == "array")
        or .error == "not_accessible" or .error == "ambiguous_window"' "${work}/elements.json"
  fi
  call clipboard_read clipboard_read
  check "clipboard_read: text or a reason" \
    '.result.structuredContent | keys == ["reason","text"] and ((.text == null) != (.reason == null))' \
    "${work}/clipboard_read.json"
  if command -v noctalia >/dev/null; then
    call shell_status shell_status
    check "shell_status: Noctalia's status, or noctalia_unavailable" \
      '.result.structuredContent | has("locked") or .error == "noctalia_unavailable"' \
      "${work}/shell_status.json"
  fi
}

check_screenshots() {
  call shot screenshot '{"target": "focused_output"}'
  check "screenshot: a JPEG image block, then its metadata as text" \
    '.result | .isError == false and .content[0].type == "image"
      and .content[0].mimeType == "image/jpeg" and (.content[0].data | length > 0)
      and (.content[1].text | fromjson) == .structuredContent' "${work}/shot.json"
  check "screenshot: at most 1280 pixels wide by default" \
    '.result.structuredContent | .width <= 1280 and .mime_type == "image/jpeg"' \
    "${work}/shot.json"
  local output
  output=$(jq -r '.result.structuredContent.output' "${work}/shot.json")
  call shot_png screenshot "{\"target\": \"output:${output}\", \"format\": \"png\", \"max_width\": 320}"
  check "screenshot: a PNG of a named output, 320 pixels wide" \
    '.result | .content[0].mimeType == "image/png" and .structuredContent.width == 320' \
    "${work}/shot_png.json"
  call shot_bad screenshot '{"target": "output:NO-SUCH-OUTPUT"}'
  check "screenshot: an unknown output is a plain-text argument mistake" \
    '.result | .isError == true and .structuredContent == null
      and (.content[0].text | startswith("invalid arguments: "))' "${work}/shot_bad.json"
}

check_audit() {
  local log="${work}/state/niri-computer-use/audit.jsonl"
  local calls
  calls=$(find "${work}" -maxdepth 1 -name '*.json' ! -name config.json ! -name tools.json \
    ! -name client.json | wc -l)
  jq -s . "${log}" >"${work}/audit.lines"
  check "audit: one line per call, from inspector-cli" \
    "length == ${calls} and all(.[]; .session | startswith(\"inspector-cli/\"))" \
    "${work}/audit.lines"
  local file_mode dir_mode
  file_mode=$(stat -c %a "${log}")
  dir_mode=$(stat -c %a "$(dirname "${log}")")
  if [[ ${file_mode} == 600 && ${dir_mode} == 700 ]]; then
    echo "ok   audit: the log is 0600 in a 0700 directory"
  else
    echo "FAIL audit: the log's modes"
    failures=$((failures + 1))
  fi
}

case "$1" in
web)
  npx -y "${INSPECTOR}" --web --config "${config}"
  ;;
*)
  check_status
  list_tools
  check_observation
  check_screenshots
  check_audit
  if [[ ${failures} -ne 0 ]]; then
    echo "${failures} checks failed"
    exit 1
  fi
  echo "all checks passed"
  ;;
esac
