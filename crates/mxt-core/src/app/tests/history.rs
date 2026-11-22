//! The histories of the `:` command line and the `/` search lines (design,
//! "Line editing").

use super::*;
use crate::effect::StoreEffect;

/// The default app with these histories loaded, each oldest first.
fn loaded(commands: &[&str], searches: &[&str]) -> App {
    let mut app = app();
    let effects = app.update(Msg::HistoryLoaded {
        commands: owned(commands),
        searches: owned(searches),
    });
    assert_eq!(effects, vec![]);
    app
}
fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}
fn save(commands: &[&str], searches: &[&str]) -> Effect {
    Effect::Store(StoreEffect::SaveHistory {
        commands: owned(commands),
        searches: owned(searches),
    })
}
/// The open line's text and cursor.
fn shown(app: &App) -> (&str, usize) {
    let line = app.command_line().unwrap();
    (line.input.text(), line.input.cursor())
}

const COMMANDS: [&str; 5] = [
    "join #a:example.org",
    "open b",
    "join #b:example.org",
    "Join x",
    "quit",
];

#[test]
fn up_recalls_the_lines_that_start_with_the_text_before_the_cursor_newest_first() {
    let mut app = loaded(&COMMANDS, &[]);
    keys(&mut app, ":jo<Up>");
    assert_eq!(shown(&app), ("join #b:example.org", 19));
    // Case-sensitive: `Join x` does not match.
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app), ("join #a:example.org", 19));
    // None left: nothing changes.
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app), ("join #a:example.org", 19));

    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("join #b:example.org", 19));
    // Past the newest match: the typed text and cursor, and browsing ends.
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("jo", 2));
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("jo", 2));
}

#[test]
fn up_on_an_empty_line_walks_every_line_and_down_walks_back() {
    let mut app = loaded(&COMMANDS, &[]);
    keys(&mut app, ":");
    for line in COMMANDS.iter().rev() {
        keys(&mut app, "<Up>");
        assert_eq!(shown(&app).0, *line);
    }
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app).0, COMMANDS[0]);
    for line in &COMMANDS[1..] {
        keys(&mut app, "<Down>");
        assert_eq!(shown(&app).0, *line);
    }
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("", 0));
}

#[test]
fn the_filter_is_the_text_before_the_cursor_which_comes_back_in_place() {
    let mut app = loaded(&COMMANDS, &[]);
    keys(&mut app, ":jox<Left><Up>");
    assert_eq!(shown(&app), ("join #b:example.org", 19));
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("jox", 2));
}

#[test]
fn an_edit_a_cursor_move_or_a_paste_ends_the_browsing() {
    let mut app = loaded(&COMMANDS, &[]);
    // Typing: the next `<Up>` filters by the text then.
    keys(&mut app, ":<Up><C-u>o<Up>");
    assert_eq!(shown(&app), ("open b", 6));
    // Moving the cursor: the filter is empty at the start.
    keys(&mut app, "<Esc>:jo<Up><Up><Home><Up>");
    assert_eq!(shown(&app), ("quit", 4));
    // A paste: `<Down>` no longer brings the typed text back.
    keys(&mut app, "<Esc>:<Up>");
    paste(&mut app, "!", None);
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("quit!", 5));
}

#[test]
fn tab_completes_a_recalled_line_and_the_history_keys_end_a_completion() {
    let mut app = loaded(&["unm", "quit"], &[]);
    keys(&mut app, ":<Up><Up>");
    assert_eq!(shown(&app), ("unm", 3));
    keys(&mut app, "<Tab>");
    assert_eq!(shown(&app), ("unmute", 6));
    // `<Tab>` ended the browsing.
    keys(&mut app, "<Down>");
    assert_eq!(shown(&app), ("unmute", 6));

    // `:u<Tab>` shows `unmute`, the first of `unmute` and `upload `. `<Up>`
    // recalls `unmute` and ends that cycle, so `<Tab>` completes `unmute`,
    // its only candidate, instead of showing `upload `.
    let mut app = loaded(&["unmute"], &[]);
    keys(&mut app, ":u<Tab>");
    assert_eq!(shown(&app), ("unmute", 6));
    keys(&mut app, "<Up><Tab>");
    assert_eq!(shown(&app), ("unmute", 6));
}

#[test]
fn commands_and_searches_have_separate_histories_and_both_panes_share_one() {
    let mut app = loaded(&["open b"], &["exa"]);
    keys(&mut app, ":<Up>");
    assert_eq!(shown(&app).0, "open b");
    keys(&mut app, "<Esc>/<Up>");
    assert_eq!(line(&app), Some((LineKind::Search(Pane::RoomList), "exa")));

    // A search of the room list, recalled in the timeline's.
    keys(&mut app, "<Esc>/A:<CR><CR>/<Up>");
    assert_eq!(line(&app), Some((LineKind::Search(Pane::Timeline), "A:")));
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app).0, "exa");
    keys(&mut app, "<Esc>:<Up>");
    assert_eq!(shown(&app).0, "open b");
}

#[test]
fn executed_lines_go_into_their_history_and_save_both() {
    let mut app = app();
    let open_b = Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!b:example.org")));
    // Recorded before the command runs.
    assert_eq!(
        keys_and_store(&mut app, ":open b<CR>", None),
        vec![save(&["open b"], &[]), open_b]
    );
    // Also an unknown command.
    assert_eq!(
        keys_and_store(&mut app, ":zzz<CR>", None),
        vec![save(&["open b", "zzz"], &[])]
    );
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    // A repeat moves to the newest place; the newest again changes nothing.
    assert_eq!(
        keys_and_store(&mut app, ":open b<CR>", None),
        vec![save(&["zzz", "open b"], &[])]
    );
    assert_eq!(keys_and_store(&mut app, ":open b<CR>", None), vec![]);
    // An empty line, a command line of blanks only, an empty search (the
    // last pattern) and a cancelled line are not recorded.
    assert_eq!(
        keys_and_store(&mut app, ":<CR>:   <CR>:help<Esc>", None),
        vec![]
    );
    keys(&mut app, "<C-w>h");
    assert_eq!(app.focus(), Focus::RoomList);
    keys(&mut app, "/b:e<CR>");
    assert_eq!(keys_and_store(&mut app, "/<CR>", None), vec![]);
    assert_eq!(cursor_id(&app), "!b:example.org");
    assert_eq!(
        keys_and_store(&mut app, "/exa<CR>", None),
        vec![save(&["zzz", "open b"], &["b:e", "exa"])]
    );
    // A search of blanks is a pattern.
    assert_eq!(
        keys_and_store(&mut app, "/  <CR>", None),
        vec![save(&["zzz", "open b"], &["b:e", "exa", "  "])]
    );
}

#[test]
fn a_completed_name_with_a_line_break_is_not_recorded() {
    let mut app = app();
    let mut broken = room("!q:example.org", 5, 0);
    broken.name.room = "q\nz".into();
    app.update(Msg::RoomUpserted(broken));
    keys(&mut app, ":open q<Tab>");
    assert_eq!(shown(&app), ("open q\nz", 8));
    // The command runs, but the history file holds one line per entry.
    assert_eq!(
        keys_and_store(&mut app, "<CR>", None),
        vec![Effect::Matrix(MatrixEffect::SubscribeTimeline(id(
            "!q:example.org"
        )))]
    );
    keys(&mut app, ":<Up>");
    assert_eq!(shown(&app), ("", 0));
}

#[test]
fn a_loaded_line_with_a_line_break_is_dropped() {
    let mut app = loaded(&["open b", "open q\nz"], &[]);
    keys(&mut app, ":<Up>");
    assert_eq!(shown(&app), ("open b", 6));
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app), ("open b", 6));
}

#[test]
fn the_101st_line_drops_the_oldest() {
    let hundred: Vec<String> = (0..100).map(|n| format!("c{n}")).collect();
    let mut app = app();
    app.update(Msg::HistoryLoaded {
        commands: hundred.clone(),
        searches: Vec::new(),
    });
    let mut commands = hundred[1..].to_vec();
    commands.push("c100".to_owned());
    assert_eq!(
        keys_and_store(&mut app, ":c100<CR>", None),
        vec![Effect::Store(StoreEffect::SaveHistory {
            commands,
            searches: Vec::new(),
        })]
    );
}

#[test]
fn a_loaded_history_keeps_its_newest_100_lines_without_repeats() {
    // `c0` to `c109`, then `c0` to `c9` again: 120 lines.
    let lines: Vec<String> = (0..110).chain(0..10).map(|n| format!("c{n}")).collect();
    let mut app = app();
    app.update(Msg::HistoryLoaded {
        commands: Vec::new(),
        searches: lines,
    });
    // `c20` moves to the newest place, and the history is saved as kept.
    let searches = (21..110)
        .chain(0..10)
        .chain([20])
        .map(|n| format!("c{n}"))
        .collect();
    assert_eq!(
        keys_and_store(&mut app, "/c20<CR>", None),
        vec![Effect::Store(StoreEffect::SaveHistory {
            commands: Vec::new(),
            searches,
        })]
    );
}

#[test]
fn a_recover_line_with_an_argument_is_not_recorded() {
    // The argument can only be the recovery key, typed or pasted after
    // the command, however it is spaced.
    let mut app = app();
    for line in [":recover abc<CR>", ": recover   abc <CR>"] {
        assert_eq!(keys_and_store(&mut app, line, None), vec![]);
        let status = app.status().unwrap();
        assert_eq!(
            (status.kind, status.text.as_str()),
            (StatusKind::Error, ":recover takes no argument")
        );
    }
    keys(&mut app, ":<Up>");
    assert_eq!(shown(&app), ("", 0));
}

#[test]
fn the_recovery_key_never_reaches_a_history() {
    let mut app = app();
    assert_eq!(
        keys_and_store(&mut app, ":recover<CR>", None),
        vec![save(&["recover"], &[])]
    );
    let key = RecoveryKey::from("key".to_owned());
    assert_eq!(
        keys_and_store(&mut app, "key<CR>", None),
        vec![Effect::Matrix(MatrixEffect::Recover(key))]
    );
    keys(&mut app, ":<Up>");
    assert_eq!(shown(&app), ("recover", 7));
    keys(&mut app, "<Up>");
    assert_eq!(shown(&app), ("recover", 7));
}

#[test]
fn up_and_down_still_move_a_pickers_selection() {
    let mut app = loaded(&COMMANDS, &[]);
    keys(&mut app, "<Space><Space><Down>");
    assert_eq!(app.picker().unwrap().selected, 1);
    keys(&mut app, "<Up>");
    let picker = app.picker().unwrap();
    assert_eq!((picker.query.text(), picker.selected), ("", 0));
}
