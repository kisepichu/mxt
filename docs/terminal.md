# Terminal setup

mxt draws with the terminal's own 16-color palette. Some features need the
terminal, or tmux, to pass things through: the clipboard, focus events and
images. This page says what each needs.

## tmux

Put these lines in `~/.tmux.conf` (or `~/.config/tmux/tmux.conf`) and
reload it:

```tmux
set -g set-clipboard on
set -g focus-events on
set -g allow-passthrough on
```

- `set-clipboard on` lets yanks reach the outer terminal's clipboard
  ([Clipboard](#clipboard)).
- `focus-events on` lets mxt know when its terminal is in front, so read
  receipts wait until you look ([Focus](#focus-and-read-receipts)).
- `allow-passthrough on` lets images through in the terminal's graphics
  protocol ([Images](#images)). With `ui.image_preview = "auto"`, mxt turns
  it on by itself (ratatui-image runs `tmux set -p allow-passthrough on`)
  for the active pane of the tmux session's current window, which is mxt's
  own pane when mxt's window is in front, and the option stays on after mxt
  exits. Setting it in the configuration makes it independent of which pane
  is active when mxt starts.

When mxt runs over SSH from inside a local tmux, the local tmux needs
`set -g allow-passthrough on` for sixel or kitty images; otherwise mxt falls
back to half blocks. `set-clipboard on` and `focus-events on` belong in the
local tmux in that case too.

If `<Esc>` feels slow inside tmux, lower tmux's `escape-time`, as for nvim.

## Clipboard

`yy` in the timeline and yanks in the composer and the message view put the
text on the terminal's clipboard with the OSC 52 escape sequence, which also
works over SSH. The terminal must accept OSC 52 clipboard writes; some turn
them off or ask first, so check its settings if yanks do not arrive. Inside
tmux, OSC 52 reaches the outer terminal only with `set -g set-clipboard on`.

mxt never reads the clipboard. `p` in the editors pastes mxt's own register,
which holds the last yank. Pasting with the terminal (its paste shortcut, or
the middle mouse button) is different: mxt turns on bracketed paste, so the
terminal hands it the pasted text as text rather than as keys. A pasted line
break stays a line break and never sends the message.

- In the composer and the message view, insert mode puts the text at the
  cursor. Normal mode puts it after the cursor, like `p`, and visual mode in
  place of the selection; `u` undoes it, and mxt's register keeps the last
  yank.
- The command line, `/` and the pickers hold one line, so they take the
  text up to its first line break.
- The room list and the timeline ignore a paste.
- A paste never opens the mention menu, even when it ends in `@name`.
- A paste larger than 256 KiB goes nowhere, and the status line says
  "Paste too large". Matrix limits a message to 64 KiB anyway.

A terminal without bracketed paste (most have it) types a paste as keys
instead: a line break in the text arrives as `<CR>`, which sends the
message in insert mode. There, paste text with several lines into the
external editor (`<C-x><C-e>`).

## Focus and read receipts

mxt sends a read receipt when the open room shows its newest message and the
terminal has focus. mxt asks the terminal to report focus changes, and counts
the terminal as focused until it reports that it lost focus. A terminal or
multiplexer that never reports focus, such as tmux without
`set -g focus-events on`, therefore counts as always focused, and receipts
go out even while you look at another window.

## Images

Images appear only in the message view (`<CR>` on an image), never in the
timeline. With `ui.image_preview = "auto"` (the default), mxt asks the
terminal for its graphics protocol at startup and draws images with
[ratatui-image](https://github.com/benjajaja/ratatui-image) in sixel, kitty
or iTerm2 graphics. When the terminal supports none of them, or does not
answer in time, ratatui-image guesses from the environment and otherwise
uses half blocks, colored half-block characters.

- mxt fetches the event's thumbnail when it has one, and the full file
  otherwise (encrypted rooms usually have no thumbnail). Files larger than
  10 MiB are not fetched; the status line says "No image preview: too large
  to preview".
- PNG, JPEG, GIF and WebP are decoded, and scaled down to the terminal.
  Decoding is limited to 128 MiB of memory, so an image with huge
  dimensions shows an error instead.
- In tmux, the query passes through tmux, so it gets no answer while mxt's
  pane is not shown by any client, or over SSH from a local tmux without
  `allow-passthrough`; mxt then guesses as above.
- With `ui.image_preview = "off"`, the view shows the image's name and size,
  and mxt never queries the terminal or changes tmux's `allow-passthrough`.

## Input methods

The composer and the message view put the terminal cursor at the editor's
cursor, so an input method shows its preedit and candidate window there,
also after wide characters and on wrapped rows, and committed text is
inserted at the cursor. In the editors' search mode the cursor is on the
bottom line instead. Text committed in the command line and in pickers is
typed into them, so `:open` and the room picker take room names in any
script.

- `<Esc>` while a preedit is open goes to the input method, not to mxt, and
  the editor stays in insert mode. Press `<Esc>` again once the preedit is
  closed.
- Outside insert mode, keys are commands (`j`, `dd`, `gx`). As in vim, turn
  the input method off there, or switch it to direct input.

## Keys

mxt reads keys in the terminal's traditional encoding, so some keys arrive
as others: `<C-i>` as `<Tab>`, `<C-m>` as `<CR>`, `<C-[>` as `<Esc>`, and in
many terminals Shift+Enter as `<CR>`. That is why a line break in the
composer is `<C-j>`. Bind the key that arrives.

## Opening files and links

`gx` runs `files.opener` (`xdg-open` on Linux, `open` on macOS) with the
downloaded file's path or the link. It runs on the machine mxt runs on: over
SSH, that is the remote machine. On WSL, `opener = ["wslview"]` opens them
in Windows. The external editor (`<C-x><C-e>`) is `$VISUAL` or `$EDITOR`;
mxt hands it the terminal while it runs.
