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
- `allow-passthrough on` lets images through to the terminal in its
  graphics protocol, which only a forced `ui.image_preview` uses inside
  tmux ([Images](#images)). Unless `ui.image_preview = "off"` or images are
  sixel that tmux stores, mxt turns it on by itself (ratatui-image runs
  `tmux set -p allow-passthrough on`) for the active pane of the tmux
  session's current window, which is mxt's own pane when mxt's window is in
  front, and the option stays on after mxt exits. Setting it in the
  configuration makes it independent of which pane is active when mxt
  starts.

When mxt runs over SSH from inside a local tmux, `set-clipboard on` and
`focus-events on` belong in the local tmux. mxt cannot tell the terminal
there, so images are usually half blocks ([Images](#images)).

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
- The recovery key prompt of `:recover` takes the text up to its first line
  break too, masked as `*` like typed text, so a key copied with a line break
  is not submitted yet; `<CR>` submits it. The text never reaches the
  composer or another input.
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
timeline. mxt draws them with
[ratatui-image](https://github.com/benjajaja/ratatui-image) in sixel, kitty
or iTerm2 graphics, or in half blocks: colored half-block characters, which
every terminal shows.

With `ui.image_preview = "auto"` (the default), mxt chooses the protocol
when it starts:

- Inside WSL, unless mxt runs over SSH (`WSL_DISTRO_NAME` is set, and
  neither `SSH_CONNECTION` nor `SSH_TTY`), images are half blocks: every
  graphics protocol breaks in ConPTY, which stands between WSL and the
  Windows terminal. mxt then asks the terminal nothing.
- Inside tmux (`TMUX` is set), images are sixel that tmux stores in the
  pane, when tmux supports it and the terminal of tmux's client draws sixel;
  otherwise they are half blocks. mxt asks tmux
  (`tmux display -p '#{version} #{sixel_support} ...'`) and nothing else.
  tmux supports it from version 3.4 when built with sixel, which only tmux
  3.6 and later report (`#{sixel_support}` is `1`), so tmux 3.4 and 3.5 get
  half blocks. The terminal draws sixel when `#{client_termfeatures}` has
  `sixel`: tmux finds it in the terminal's device attributes, and for a
  terminal where it does not, `set -as terminal-features ',xterm*:sixel'`
  adds it. tmux also needs the size of the terminal's cells in pixels
  (`#{client_cell_width}` and `#{client_cell_height}`, which it gets from
  the terminal's window size or asks the terminal for); without it tmux
  shows a placeholder, so images are half blocks. tmux places such an image
  in the pane, keeps it within the pane, redraws it with the pane, and
  removes it once mxt draws over it. Any other image goes through tmux to
  the terminal, which draws it at its own cursor, without the pane's
  offset, and tmux neither moves nor removes it.
- Otherwise mxt asks the terminal for its name (the XTVERSION query, which
  also works over SSH).

| Terminal | Protocol |
|---|---|
| WezTerm, iTerm2 | iTerm2 |
| kitty, Ghostty | kitty |
| any other, or no answer | ratatui-image's choice |

ratatui-image asks the terminal which protocols it supports and how large
a cell is in pixels. When the answers do not come in time, it guesses from
the environment (iTerm2 hints, the cell size from the window's size in
pixels), and with neither it uses half blocks. mxt uses the protocol from
the table only when the cell size is known, as ratatui-image does, since the
graphics protocols draw an image in pixels that must fill its cells.

- WezTerm gets iTerm2 graphics: ratatui-image's kitty graphics use Unicode
  placeholders, which WezTerm draws as boxes, and over SSH ratatui-image
  cannot tell WezTerm by itself.
- Windows Terminal does not answer the XTVERSION query. ratatui-image
  finds sixel support in its device attributes.
- Over SSH from inside a local tmux, the local tmux answers for the
  terminal, and the remote mxt is not inside tmux, so it cannot ask tmux.
  ratatui-image's choice there is usually half blocks.
- With several tmux clients attached, tmux reports the features of one of
  them, the one it takes for mxt's session, and mxt decides by that one.
  tmux draws a sixel image only for clients whose terminal draws sixel, and
  shows the others a placeholder (`SIXEL IMAGE (width x height)`) in its
  place. tmux counts an image's rows in the cell size of the client that
  sets the window's size: with tmux's default `window-size latest`, the
  client that attached or typed last. When a client with another window
  size takes over, tmux resizes the window, and mxt draws images again in
  the new cell size. In a window split into panes, mxt's pane may keep its
  size; mxt then keeps the old cell size until the pane resizes again, and
  an image may take more or fewer rows than its area, and scroll the pane
  when it reaches past the pane's bottom.

When the choice is wrong for your terminal, `ui.image_preview = "iterm2"`,
`"sixel"`, `"kitty"` or `"halfblocks"` uses that protocol, also in WSL.
Inside tmux, `"sixel"` is the sixel that tmux stores wherever tmux supports
it, whatever tmux says about the terminal. `"iterm2"`, `"kitty"`, and
`"sixel"` where tmux does not support it, pass through tmux (see
`allow-passthrough` [above](#tmux)), so inside tmux these images may be
drawn in the wrong place and leave pieces behind. mxt then skips the name,
and except for tmux's sixel, ratatui-image's query still runs for the cell
size; when it gets none, ratatui-image assumes 10 by 20 pixels, and images
in a graphics protocol may be drawn larger or smaller than their area. In
WSL, any of these values, `"halfblocks"` too, brings back the query that
`"auto"` skips there: when ConPTY does not pass the answers on, mxt starts
2 seconds later, and ratatui-image's reader stays waiting on the terminal,
where it may take your first key press.

- mxt fetches the event's thumbnail when it has one, and the full file
  otherwise (encrypted rooms usually have no thumbnail). Files larger than
  10 MiB are not fetched; the status line says "No image preview: too large
  to preview".
- PNG, JPEG, GIF and WebP are decoded, and scaled down to the terminal.
  Decoding is limited to 128 MiB of memory, so an image with huge
  dimensions shows an error instead.
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
