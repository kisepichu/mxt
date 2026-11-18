#!/usr/bin/env bash
# Phase 0 edtui CJK check (throwaway, never merged).
#
# Builds edtui-cjk, runs it in a new window of tmux session `mxt`, drives it
# with `tmux send-keys`, and compares the editor state (the JSON the program
# writes) and the terminal cursor (`#{cursor_x}`, `#{cursor_y}`) with the
# expected values. `send-keys -l` with Japanese text stands in for text
# committed by an input method; preedit (composition) display cannot be
# checked from tmux and stays a human check.
#
# Prints PASS, FAIL or RECORD (behavior recorded, no fixed expectation) per
# scenario. Exits 1 if any scenario fails.
set -euo pipefail
# `${#text}` must count characters, not bytes.
export LC_ALL=C.UTF-8

session=mxt
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)

for tool in tmux jq nix; do
  command -v "$tool" >/dev/null || { echo "check.sh: $tool not found" >&2; exit 2; }
done
tmux has-session -t "=$session" 2>/dev/null ||
  { echo "check.sh: tmux session $session not found" >&2; exit 2; }

(cd "$root" && nix develop -c cargo build -p edtui-cjk)
bin=${CARGO_TARGET_DIR:-$root/target}/debug/edtui-cjk

workdir=$(mktemp -d "${TMPDIR:-/tmp}/edtui-cjk.XXXXXX")
out=$workdir/state.json

# `$session:` (with the colon) is always the session, never a window of the
# current session that happens to be called `mxt`. The placeholder `cat`
# keeps the pane alive until the first scenario respawns it.
pane=$(tmux new-window -d -t "$session:" -n cjk-check -P -F '#{pane_id}' cat)
window=$(tmux display -p -t "$pane" '#{window_id}')
cleanup() {
  tmux kill-window -t "$window" 2>/dev/null || true
  rm -rf "$workdir"
}
trap cleanup EXIT
tmux set -w -t "$window" remain-on-exit on
tmux set -w -t "$window" allow-set-title on

width=$(tmux display -p -t "$pane" '#{pane_width}')
echo "tmux: $(tmux -V)"
echo "pane: $(tmux display -p -t "$pane" '#{pane_width}x#{pane_height}')"
echo "edtui: $(grep -A1 '^name = "edtui"$' "$root/Cargo.lock" | sed -n 's/^version = "\(.*\)"/\1/p')"

failed=()

# poll <seconds> <command...>: run the command every 10 ms until it succeeds.
poll() {
  local tries=$(($1 * 100)) i
  shift
  for ((i = 0; i < tries; i++)); do
    "$@" && return 0
    sleep 0.01
  done
  return 1
}

title() { tmux display -p -t "$pane" '#{pane_title}'; }
cursor() { tmux display -p -t "$pane" '#{cursor_x},#{cursor_y}'; }
file_keys() { jq -r '.keys' "$out" 2>/dev/null || true; }

# The program writes the file after the frame for its `keys`-th key, and sets
# the title right after that frame's cursor movement. Both must show the
# number of keys sent so far.
frame_shown() { [ "$(file_keys)" = "$keys" ] && [ "$(title)" = "edtui-cjk:$keys" ]; }
wait_frame() {
  if ! poll 5 frame_shown; then
    printf '  FAIL  timed out waiting for key %s (file keys=%s, title=%s)\n' \
      "$keys" "$(file_keys)" "$(title)"
    ok=0
  fi
}

expect() { # expect <what> <actual> <wanted>
  if [ "$2" = "$3" ]; then
    printf '  ok    %s: %s\n' "$1" "$2"
  else
    printf '  FAIL  %s: got %s, want %s\n' "$1" "$2" "$3"
    ok=0
  fi
}
expect_json() { expect "$1" "$(jq -c "$2" "$out")" "$3"; }

# After every key, print the state and check, unless turned off:
# - check_cursor: the terminal cursor is at (display_col, row), which holds
#   while no line wraps;
# - check_cell: the cell drawn at the cursor holds the character under the
#   cursor (or is blank after the end of the line in insert mode).
step() {
  local mode row col dcol screen char cell cell_ok cur
  {
    read -r mode
    read -r row
    read -r col
    read -r dcol
    read -r screen
    read -r char
    read -r cell
    read -r cell_ok
  } < <(jq -r '.mode, .row, .col, .display_col,
    (.screen // [] | map(tostring) | join(",")), (.char | tojson), (.cell | tojson),
    (.char as $c | if $c == null then .cell == " " else (.cell // "" | startswith($c)) end)' "$out")
  cur=$(cursor)
  printf '  after %-14s mode=%s row=%s col=%s display_col=%s char=%s cell=%s edtui=(%s) tmux=(%s)\n' \
    "$1" "$mode" "$row" "$col" "$dcol" "$char" "$cell" "$screen" "$cur"
  if [ "$check_cursor" = 1 ] && [ "$cur" != "$dcol,$row" ]; then
    printf '  FAIL  cursor drawn at (%s), want (%s,%s)\n' "$cur" "$dcol" "$row"
    ok=0
  fi
  if [ "$check_cell" = 1 ] && [ "$cell_ok" != true ]; then
    printf '  FAIL  cell at the cursor holds %s, want %s\n' "$cell" "$char"
    ok=0
  fi
}
key() { # key <tmux key name>...: one key event each
  local k
  for k in "$@"; do
    tmux send-keys -t "$pane" "$k"
    keys=$((keys + 1))
    wait_frame
    step "<$k>"
  done
}
typed() { # typed <text>: one key event per character, like committed IME text
  local label="\"$1\""
  if ((${#1} > 12)); then label="\"${1:0:6}...\"(${#1})"; fi
  tmux send-keys -t "$pane" -l -- "$1"
  keys=$((keys + ${#1}))
  wait_frame
  step "$label"
}

capture() {
  echo "  pane (non-empty rows):"
  tmux capture-pane -p -t "$pane" | awk '$0 != "" { printf "  | %2d %s\n", NR - 1, $0 }'
}
screen_row() { tmux capture-pane -p -t "$pane" -S "$1" -E "$1" | sed 's/ *$//'; }
repeat() { local i s=; for ((i = 0; i < $2; i++)); do s+=$1; done; printf '%s' "$s"; }

begin() { # begin <name> <description>: fresh program, empty buffer
  name=$1
  ok=1
  check_cursor=1
  check_cell=1
  printf '\n== Scenario %s: %s\n' "$1" "$2"
  rm -f "$out"
  tmux respawn-pane -k -t "$pane" "$bin" --out "$out"
  keys=0
  wait_frame
}
end() { # end [RECORD]
  local verdict=PASS
  [ "${1:-}" = RECORD ] && verdict=RECORD
  if [ "$ok" = 0 ]; then
    verdict=FAIL
    failed+=("$name")
  fi
  echo "$verdict $name"
  if [ "$verdict" != PASS ]; then capture; fi
}

hello=こんにちは世界
scenario1_keys() { key i; typed "$hello"; key Escape; }

begin 1 "i, $hello, <Esc>"
key i
typed "$hello"
expect_json "insert mode col after the committed text" .col 7
key Escape
expect_json text .lines "[\"$hello\"]"
expect_json mode .mode '"Normal"'
expect_json col .col 6
expect_json display_col .display_col 12
expect cursor "$(cursor)" 12,0
end

begin 2 "scenario 1, then 0, l, l"
scenario1_keys
typed 0
typed l
typed l
expect_json col .col 2
expect_json display_col .display_col 4
expect cursor "$(cursor)" 4,0
end

begin 3 "scenario 1, then 0, x"
scenario1_keys
typed 0
typed x
expect_json text .lines '["んにちは世界"]'
expect_json col .col 0
expect cursor "$(cursor)" 0,0
end

begin 4 "scenario 1, then 0, w, e, b (where each motion lands)"
scenario1_keys
typed 0
typed w
typed e
typed b
end RECORD

begin 4b "i, abc日本語def, <Esc>, 0, w, w, e, e, b, b (word motions across ASCII and CJK)"
key i
typed abc日本語def
key Escape
typed 0
typed w
typed w
typed e
typed e
typed b
typed b
end RECORD

begin 5 "i, abc日本語def, <Esc>, F, 本"
key i
typed abc日本語def
key Escape
typed F
typed 本
expect_json col .col 4
expect_json display_col .display_col 5
expect cursor "$(cursor)" 5,0
end

begin 5b "i, abc日本語def, <Esc>, 0, f, 本 (forward find, to separate F from CJK)"
key i
typed abc日本語def
key Escape
typed 0
typed f
typed 本
expect_json col .col 4
expect_json display_col .display_col 5
expect cursor "$(cursor)" 5,0
end

begin 6 "i, 日本語, <Left>, <Left>, X, <Esc>"
key i
typed 日本語
key Left Left
typed X
key Escape
expect_json text .lines '["日X本語"]'
expect_json col .col 1
expect cursor "$(cursor)" 2,0
end

# A line 1.5 times the pane width. With an even width, a leading `a` makes
# the first row end one column short, so the next あ straddles the boundary.
scenario7() { # scenario7 <name>
  local width prefix count first rest line
  width=$(tmux display -p -t "$pane" '#{pane_width}')
  if ((width % 2 == 0)); then prefix=a; else prefix=; fi
  count=$((width * 3 / 4))
  first=$(((width - ${#prefix}) / 2)) # あ that fit on the first row
  rest=$((count - first))
  line=$prefix$(repeat あ "$count")
  begin "$1" "i, ${prefix:+a + }$count あ ($((${#prefix} + 2 * count)) columns in a $width-column pane), <Esc>"
  check_cursor=0
  key i
  typed "$line"
  expect_json text .lines "[\"$line\"]"
  expect "insert cursor after the text" "$(cursor)" "$((2 * rest)),1"
  key Escape
  expect_json col .col $((${#prefix} + count - 1))
  expect "cursor on the last あ" "$(cursor)" "$((2 * (rest - 1))),1"
  # The last column of row 0 stays empty and the next あ starts row 1 whole.
  expect "screen row 0" "$(screen_row 0)" "$prefix$(repeat あ "$first")"
  expect "screen row 1" "$(screen_row 1)" "$(repeat あ "$rest")"
  end
}
scenario7 7
# The other width parity: one column narrower.
tmux resize-window -t "$window" -x $((width - 1))
scenario7 7b
tmux resize-window -t "$window" -x "$width"

emoji=$'\U1F44D\U1F3FD and \U1F468\u200D\U1F469\u200D\U1F467'
begin 8 "i, $emoji (${#emoji} chars), <Esc>, 0, l, l"
check_cursor=0
check_cell=0
key i
typed "$emoji"
key Escape
typed 0
typed l
typed l
expect_json text .lines "[\"$emoji\"]"
end RECORD

# Variation selector 16 (emoji presentation), common in chat: ❤️ is two chars.
heart=$'\u2764\uFE0F ok'
begin 8b "i, $heart (${#heart} chars), <Esc>, 0, l, \$"
check_cursor=0
check_cell=0
key i
typed "$heart"
key Escape
typed 0
typed l
typed '$'
expect_json text .lines "[\"$heart\"]"
end RECORD

begin 9 "scenario 1, then y, y, p"
scenario1_keys
typed y
typed y
typed p
expect_json text .lines "[\"$hello\",\"$hello\"]"
end

begin 9b "i, あ, <Enter>, い, <Esc>, k, y, y, j, p (p on the last line; not CJK-specific)"
key i
typed あ
key Enter
typed い
key Escape
typed k
typed y
typed y
typed j
typed p
# vim puts the line below the cursor line.
expect_json text .lines '["あ","い","あ"]'
end

begin Q "<C-q> quits and restores the terminal"
scenario1_keys
tmux send-keys -t "$pane" C-q
pane_dead() { [ "$(tmux display -p -t "$pane" '#{pane_dead}')" = 1 ]; }
if poll 5 pane_dead; then
  expect "exit status" "$(tmux display -p -t "$pane" '#{pane_dead_status}')" 0
  expect "alternate screen" "$(tmux display -p -t "$pane" '#{alternate_on}')" 0
else
  expect "program exited" no yes
fi
end

echo
if ((${#failed[@]})); then
  echo "FAILED: ${failed[*]}"
  exit 1
fi
echo "ALL PASSED"
