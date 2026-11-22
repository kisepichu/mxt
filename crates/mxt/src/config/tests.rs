use super::*;
use mxt_core::action::Action;
use mxt_core::keymap::InputContext;
use mxt_core::keys::KeySeq;
use mxt_core::rooms::{RoomSort, SortKey};
use mxt_core::settings::{ImagePreview, ImageProtocol, ReceiptKind};
use mxt_tui::NetworkPlacement;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

/// The configuration, which must have no warnings.
fn ok(text: &str) -> Config {
    let (config, warnings) = warned(text);
    assert!(warnings.is_empty(), "{warnings:#?}");
    config
}
/// The configuration and its warnings, each as its line, column and message.
fn warned(text: &str) -> (Config, Vec<(usize, usize, String)>) {
    let config = parse(text, Path::new("config.toml"), Path::new("/home/alice"))
        .unwrap_or_else(|e| panic!("{e}"));
    let warnings = config
        .warnings
        .iter()
        .map(|warning| (warning.line, warning.column, warning.message.clone()))
        .collect();
    (config, warnings)
}
/// The one warning of a configuration.
fn warning(text: &str) -> (usize, usize, String) {
    let (_, mut warnings) = warned(text);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    warnings.remove(0)
}
fn err(text: &str) -> (usize, String) {
    let (line, _, message) = err_at(text);
    (line, message)
}
/// The line, the column and the message.
fn err_at(text: &str) -> (usize, usize, String) {
    match parse(text, Path::new("config.toml"), Path::new("/home/alice")) {
        Err(ConfigError::Invalid {
            line,
            column,
            message,
            ..
        }) => (line, column, message),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("accepted: {text}"),
    }
}
fn lookup(c: &Config, ctx: InputContext, keys: &str) -> Option<Vec<&'static str>> {
    let leader = KeySeq::parse("<Space>", &KeySeq::default()).unwrap();
    let seq = KeySeq::parse(keys, &leader).unwrap();
    c.keymap
        .effective(ctx)
        .lookup(seq.keys())
        .binding
        .map(|b| b.actions().iter().map(Action::name).collect())
}

const MINIMAL: &str = "[account]\nuser_id = \"@alice:example.org\"\n";

#[test]
fn minimal_file_uses_defaults() {
    let c = ok(MINIMAL);
    assert_eq!(c.user_id.as_str(), "@alice:example.org");
    assert_eq!(c.homeserver, None);
    assert_eq!(c.settings.timeoutlen_ms, 1000);
    assert_eq!(c.settings.room_sort, RoomSort::one(SortKey::Recent));
    assert!(c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Auto);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Public);
    assert_eq!(c.time_format, TimeFormat::default());
    assert_eq!(
        c.room_list_layout,
        RoomListLayout {
            width: RoomListWidth::new(30).unwrap(),
            network: NetworkPlacement::Inline,
        }
    );
    assert_eq!(c.download_dir, PathBuf::from("/home/alice/Downloads"));
    assert!(!c.opener.is_empty());
    assert_eq!(
        lookup(&c, InputContext::Timeline, "j"),
        Some(vec!["cursor.down"])
    );
}

#[test]
fn the_design_example_loads() {
    let c = ok(r#"
[account]
user_id = "@alice:example.org"
# homeserver = "https://matrix.example.org"

[ui]
leader = "<Space>"
timeoutlen = 1000
room_list = { visible = true, sort = "recent", width = 30, network = "inline" }
time_format = "%H:%M"
image_preview = "auto"
read_receipts = "public"

[files]
download_dir = "~/Downloads"
opener = ["xdg-open"]

[keymap.normal]
"]n" = { action = "room.open", filter = "notify", pick = "next" }
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]

[keymap.timeline]
"dd" = "nop"
"D" = "message.delete"

[keymap.composer.insert]
"<C-s>" = "composer.send"
"#);
    assert_eq!(
        lookup(&c, InputContext::Timeline, "]n"),
        Some(vec!["room.open"])
    );
    assert_eq!(
        lookup(&c, InputContext::RoomList, "<Space>x"),
        Some(vec!["room.mark_read", "room.open"])
    );
    assert_eq!(lookup(&c, InputContext::Timeline, "dd"), Some(vec!["nop"]));
    assert_eq!(
        lookup(&c, InputContext::Timeline, "D"),
        Some(vec!["message.delete"])
    );
    assert_eq!(
        lookup(&c, InputContext::ComposerInsert, "<C-s>"),
        Some(vec!["composer.send"])
    );
    assert_eq!(
        lookup(&c, InputContext::ComposerInsert, "<CR>"),
        Some(vec!["composer.send"])
    );
}

#[test]
fn a_binding_in_the_thread_table_applies_only_in_a_thread() {
    let c = ok(&format!(
        "{MINIMAL}[keymap.thread]\n\"x\" = \"message.yank\"\n\"q\" = \"nop\"\n"
    ));
    assert_eq!(
        lookup(&c, InputContext::Thread, "x"),
        Some(vec!["message.yank"])
    );
    assert_eq!(lookup(&c, InputContext::Thread, "q"), Some(vec!["nop"]));
    // The rest falls back to the timeline's and normal's bindings.
    assert_eq!(
        lookup(&c, InputContext::Thread, "t"),
        Some(vec!["thread.open"])
    );
    assert_eq!(
        lookup(&c, InputContext::Thread, ":"),
        Some(vec!["command_line.open"])
    );
    assert_eq!(lookup(&c, InputContext::Timeline, "x"), None);
}

#[test]
fn settings_are_read() {
    let c = ok(&format!(
        "{MINIMAL}[ui]\ntimeoutlen = 300\nread_receipts = \"private\"\nroom_list = {{ visible = false, sort = \"name\", width = 40, network = \"below\" }}\nimage_preview = \"off\"\n[files]\ndownload_dir = \"/tmp/dl\"\nopener = [\"wslview\"]\n"
    ));
    assert_eq!(c.settings.timeoutlen_ms, 300);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Private);
    assert_eq!(c.settings.room_sort, RoomSort::one(SortKey::Name));
    assert!(!c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Off);
    assert_eq!(
        c.room_list_layout,
        RoomListLayout {
            width: RoomListWidth::new(40).unwrap(),
            network: NetworkPlacement::Below,
        }
    );
    assert_eq!(c.download_dir, PathBuf::from("/tmp/dl"));
    assert_eq!(c.opener, vec!["wslview".to_string()]);
}

#[test]
fn image_preview_takes_auto_off_and_each_protocol() {
    for (value, preview) in [
        ("auto", ImagePreview::Auto),
        ("off", ImagePreview::Off),
        ("iterm2", ImagePreview::Forced(ImageProtocol::Iterm2)),
        ("sixel", ImagePreview::Forced(ImageProtocol::Sixel)),
        ("kitty", ImagePreview::Forced(ImageProtocol::Kitty)),
        (
            "halfblocks",
            ImagePreview::Forced(ImageProtocol::Halfblocks),
        ),
    ] {
        let c = ok(&format!("{MINIMAL}[ui]\nimage_preview = \"{value}\"\n"));
        assert_eq!(c.settings.image_preview, preview, "{value}");
    }
    for value in ["chafa", "Kitty", "iTerm2", "half_blocks", ""] {
        let text = format!("{MINIMAL}[ui]\nimage_preview = \"{value}\"\n");
        let (c, warnings) = warned(&text);
        assert_eq!(c.settings.image_preview, ImagePreview::Auto, "{value}");
        let [(line, column, msg)] = warnings.as_slice() else {
            panic!("{warnings:#?}");
        };
        assert_eq!((*line, *column), (4, 17), "{value}");
        assert!(
            msg.starts_with("ui.image_preview: ") && msg.contains("halfblocks"),
            "{msg}"
        );
    }
}

#[test]
fn homeserver_and_time_format_are_read() {
    let c = ok(&format!(
        "{MINIMAL}homeserver = \"https://matrix.example.org\"\n[ui]\ntime_format = \"%Y-%m-%d %H:%M\"\n"
    ));
    assert_eq!(c.homeserver.as_deref(), Some("https://matrix.example.org"));
    assert_eq!(
        c.time_format,
        TimeFormat::parse("%Y-%m-%d %H:%M".to_owned()).unwrap()
    );
}

#[test]
fn an_invalid_time_format_names_its_line() {
    let (line, column, msg) = warning(&format!(
        "{MINIMAL}[ui]\ntimeoutlen = 300\ntime_format = \"%H:%K\"\n"
    ));
    assert_eq!((line, column), (5, 15));
    assert!(msg.starts_with("ui.time_format: "), "{msg}");
    let (line, msg) = err(&format!("{MINIMAL}[ui]\ntime_format = \"\"\n"));
    assert_eq!(line, 4);
    assert_eq!(msg, "ui.time_format: must not be empty");
}

#[test]
fn the_room_list_width_is_from_12_to_80() {
    for width in [12, 80] {
        let c = ok(&format!(
            "{MINIMAL}[ui]\nroom_list = {{ width = {width} }}\n"
        ));
        assert_eq!(c.room_list_layout.width.columns(), width);
    }
    for width in ["11", "81", "0", "-30", "70000"] {
        let text = format!("{MINIMAL}[ui]\nroom_list = {{ width = {width} }}\n");
        assert_eq!(
            err_at(&text),
            (
                4,
                23,
                "ui.room_list.width: must be from 12 to 80".to_owned()
            ),
            "{width}"
        );
    }
}

#[test]
fn a_homeserver_needs_a_scheme() {
    let (line, msg) = err(&format!("{MINIMAL}homeserver = \"matrix.example.org\"\n"));
    assert_eq!(line, 3);
    assert!(msg.contains("account.homeserver"), "{msg}");
}

#[test]
fn the_download_dir_must_be_absolute() {
    for value in ["~alice/dl", "Downloads", ""] {
        let (line, msg) = err(&format!("{MINIMAL}[files]\ndownload_dir = {value:?}\n"));
        assert_eq!(line, 4, "{value:?}");
        assert!(
            msg.contains("files.download_dir") && msg.contains(&format!("{value:?}")),
            "{msg}"
        );
    }
    let c = ok(&format!("{MINIMAL}[files]\ndownload_dir = \"~/dl\"\n"));
    assert_eq!(c.download_dir, PathBuf::from("/home/alice/dl"));
}

#[test]
fn argument_errors_point_at_the_argument() {
    // A binding written as a `[header]` table spans only its header line.
    let header =
        |args: &str| format!("{MINIMAL}[keymap.normal.\"]u\"]\naction = \"room.open\"\n{args}");
    for (args, line) in [
        // A wrong type.
        ("filter = 3\n", 5),
        // A conflict: the later of the two in the file.
        ("pick = \"next\"\nfilter = \"all\"\nquery = \"x\"\n", 7),
        ("query = \"x\"\npick = \"next\"\n", 6),
    ] {
        assert_eq!(err(&header(args)).0, line, "{args}");
    }
    // Warnings too.
    for (args, line) in [
        // An unknown argument.
        ("filter = \"unread\"\npik = \"next\"\n", 6),
        // An unknown value.
        ("pick = \"next\"\nfilter = \"unred\"\n", 6),
    ] {
        assert_eq!(warning(&header(args)).0, line, "{args}");
    }
    // Errors about the action as a whole stay at the binding.
    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.normal.x]\naction = \"file.upload\"\n"
    ));
    assert_eq!(line, 3);
    assert!(msg.contains("path"), "{msg}");
}

#[test]
fn an_unknown_emoji_points_at_its_argument() {
    let (line, _, msg) = warning(&format!(
        "{MINIMAL}[keymap.timeline.t]\naction = \"message.react\"\nemoji = \"nonsense\"\n"
    ));
    assert_eq!(line, 5);
    assert!(msg.contains("unknown emoji \"nonsense\""), "{msg}");
}

#[test]
fn leader_applies_to_defaults_and_user_bindings() {
    let c = ok(&format!(
        "{MINIMAL}[ui]\nleader = \",\"\n[keymap.normal]\n\"<leader>q\" = \"app.quit\"\n"
    ));
    let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
    let seq = KeySeq::parse("<leader>q", &comma).unwrap();
    let names: Vec<_> = c
        .keymap
        .effective(InputContext::Timeline)
        .lookup(seq.keys())
        .binding
        .unwrap()
        .actions()
        .iter()
        .map(Action::name)
        .collect();
    assert_eq!(names, vec!["app.quit"]);
    let e = KeySeq::parse("<leader>e", &comma).unwrap();
    assert!(
        c.keymap
            .effective(InputContext::Timeline)
            .lookup(e.keys())
            .binding
            .is_some()
    );
}

#[test]
fn errors_and_warnings_name_the_line() {
    let (line, _, msg) = warning(&format!("{MINIMAL}[ui]\ntimeoutln = 5\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("timeoutln"), "{msg}");

    let (line, _, msg) = warning(&format!(
        "{MINIMAL}[keymap.timeline]\n\"x\" = \"message.dlete\"\n"
    ));
    assert_eq!(line, 4);
    assert!(msg.contains("message.dlete"), "{msg}");

    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.normal]\n\"]u\" = {{ action = \"room.open\", pick = \"next\", query = \"x\" }}\n"
    ));
    assert_eq!(line, 4);
    assert!(msg.contains("pick") && msg.contains("query"), "{msg}");

    let (line, msg) = err(&format!("{MINIMAL}[keymap.normal]\n\"<C-x\" = \"nop\"\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("<C-x"), "{msg}");

    let (line, _) = err(&format!("{MINIMAL}[keymap.normal]\n\"x\" = []\n"));
    assert_eq!(line, 4);

    let (line, _, _) = warning(&format!(
        "{MINIMAL}[keymap.composer.foo]\n\"x\" = \"nop\"\n"
    ));
    assert_eq!(line, 3);

    let (line, _) = err(&format!(
        "{MINIMAL}[keymap.normal]\n\"x\" = {{ action = \"room.open\", filter = 3.5 }}\n"
    ));
    assert_eq!(line, 4);

    let (line, msg) = err("[account]\nuser_id = \"alice\"\n");
    assert_eq!(line, 2);
    assert!(msg.contains("user_id"), "{msg}");

    let (line, _) = err("[account]\n");
    assert_eq!(line, 1);
}

#[test]
fn messages_name_the_setting() {
    let (line, msg) = err(&format!("{MINIMAL}[ui]\ntimeoutlen = \"1s\"\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("ui.timeoutlen"), "{msg}");

    let (line, _, msg) = warning(&format!(
        "{MINIMAL}[ui]\nroom_list = {{ sort = \"size\" }}\n"
    ));
    assert_eq!(line, 4);
    assert!(
        msg.contains("ui.room_list.sort") && msg.contains("recent"),
        "{msg}"
    );

    let (line, column, msg) = warning(&format!(
        "{MINIMAL}[ui]\nroom_list = {{ network = \"beside\" }}\n"
    ));
    assert_eq!((line, column), (4, 25));
    assert!(
        msg.contains("ui.room_list.network") && msg.contains("below"),
        "{msg}"
    );

    let (line, msg) = err(&format!("{MINIMAL}[files]\nopener = []\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("files.opener"), "{msg}");

    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.normal]\n\"x\" = [\"nop\", {{ filter = \"all\" }}]\n"
    ));
    assert_eq!(line, 4);
    assert!(
        msg.contains("keymap.normal.x") && msg.contains("action"),
        "{msg}"
    );
}

#[test]
fn two_spellings_of_one_key_sequence_are_rejected() {
    // The first spelling sorts after the second, so the error must follow
    // the file order to land on the second key's line.
    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.normal]\n\"<leader>x\" = \"app.quit\"\n\"<Space>x\" = \"nop\"\n"
    ));
    assert_eq!(line, 5);
    assert!(
        msg.contains("<leader>x") && msg.contains("<Space>x"),
        "{msg}"
    );

    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.timeline]\n\"<C-a>\" = \"nop\"\n\"<C-A>\" = \"app.quit\"\n"
    ));
    assert_eq!(line, 5);
    assert!(msg.contains("<C-a>") && msg.contains("<C-A>"), "{msg}");

    // The same keys in different tables are separate bindings.
    let c = ok(&format!(
        "{MINIMAL}[keymap.normal]\n\"<C-a>\" = \"nop\"\n[keymap.timeline]\n\"<C-A>\" = \"app.quit\"\n"
    ));
    assert_eq!(
        lookup(&c, InputContext::Timeline, "<C-a>"),
        Some(vec!["app.quit"])
    );
    assert_eq!(
        lookup(&c, InputContext::RoomList, "<C-a>"),
        Some(vec!["nop"])
    );
}

#[test]
fn a_binding_identical_to_a_default_is_accepted() {
    // Bindings that configurations had before they became defaults.
    let bindings = r#"
[keymap.normal]
"<C-w><C-h>" = "focus.room_list"
"<C-w><C-l>" = "focus.timeline"
"<C-w>j" = { action = "composer.focus", mode = "keep" }
"<C-w><C-j>" = { action = "composer.focus", mode = "keep" }
"<C-w>k" = "focus.timeline"
"<C-w><C-k>" = "focus.timeline"
"]u" = { action = "room.open", filter = "unread", pick = "next" }
"[u" = { action = "room.open", filter = "unread", pick = "prev" }

[keymap.picker]
"<Down>" = "picker.next"
"<Tab>" = "picker.next"
"<Up>" = "picker.prev"
"<S-Tab>" = "picker.prev"

[keymap.composer.completion]
"<Down>" = "completion.next"
"<Up>" = "completion.prev"
"#;
    let c = ok(&format!("{MINIMAL}{bindings}"));
    let defaults = ok(MINIMAL);
    let leader = KeySeq::parse("<Space>", &KeySeq::default()).unwrap();
    for (ctx, keys) in [
        (InputContext::RoomList, "<C-w><C-h>"),
        (InputContext::Timeline, "<C-w><C-l>"),
        (InputContext::Timeline, "<C-w>j"),
        (InputContext::Timeline, "<C-w><C-j>"),
        (InputContext::ComposerNormal, "<C-w>k"),
        (InputContext::ComposerNormal, "<C-w><C-k>"),
        (InputContext::Timeline, "]u"),
        (InputContext::RoomList, "[u"),
        (InputContext::Picker, "<Down>"),
        (InputContext::Picker, "<Tab>"),
        (InputContext::Picker, "<Up>"),
        (InputContext::Picker, "<S-Tab>"),
        (InputContext::ComposerCompletion, "<Down>"),
        (InputContext::ComposerCompletion, "<Up>"),
    ] {
        let seq = KeySeq::parse(keys, &leader).unwrap();
        let binding = c.keymap.effective(ctx).get(seq.keys());
        assert!(binding.is_some(), "{keys}");
        assert_eq!(
            binding,
            defaults.keymap.effective(ctx).get(seq.keys()),
            "{keys}"
        );
    }
}

/// The warnings as `warned` gives them, from string slices.
fn expected(warnings: &[(usize, usize, &str)]) -> Vec<(usize, usize, String)> {
    warnings
        .iter()
        .map(|&(line, column, message)| (line, column, message.to_owned()))
        .collect()
}

#[test]
fn unknown_keys_and_tables_are_ignored_with_a_warning() {
    let (c, warnings) = warned(
        r#"[account]
user_id = "@alice:example.org"
[ui]
font = "Iosevka"
timeoutlen = 300
room_list = { visible = false, rows = 2 }
[notifications]
sound = true
[keymap.composer.foo]
"x" = "nop"
[keymap.foo]
"y" = "nop"
[keymap.timeline]
"D" = "message.delete"
"#,
    );
    assert_eq!(
        warnings,
        expected(&[
            (4, 1, "ui.font: unknown key (ignored)"),
            (6, 32, "ui.room_list.rows: unknown key (ignored)"),
            (7, 2, "notifications: unknown table (ignored)"),
            (9, 18, "keymap.composer.foo: unknown table (ignored)"),
            (11, 9, "keymap.foo: unknown table (ignored)"),
        ])
    );
    assert_eq!(c.settings.timeoutlen_ms, 300);
    assert!(!c.settings.room_list_visible);
    assert_eq!(
        lookup(&c, InputContext::Timeline, "D"),
        Some(vec!["message.delete"])
    );
}

#[test]
fn unknown_setting_values_keep_the_default_but_read_receipts_turn_private() {
    let text = r#"[account]
user_id = "@alice:example.org"
[ui]
image_preview = "wezterm"
read_receipts = "x"
time_format = "%H:%K"
room_list = { sort = "size", network = "beside" }
"#;
    let (c, warnings) = warned(text);
    let jiff = TimeFormat::parse("%H:%K".to_owned()).unwrap_err();
    assert_eq!(
        warnings,
        expected(&[
            (
                4,
                17,
                "ui.image_preview: unknown value \"wezterm\"; expected one of \"auto\", \"off\", \
                 \"iterm2\", \"sixel\", \"kitty\", \"halfblocks\" (ignored)"
            ),
            (
                5,
                17,
                "ui.read_receipts: unknown value \"x\", using \"private\""
            ),
            (6, 15, &format!("ui.time_format: {jiff} (ignored)")),
            (
                7,
                22,
                "ui.room_list.sort: unknown value \"size\"; expected one of \"mentions\", \
                 \"notify\", \"unread\", \"recent\", \"read\", \"name\" (ignored)"
            ),
            (
                7,
                40,
                "ui.room_list.network: unknown value \"beside\"; expected one of \"inline\", \
                 \"below\" (ignored)"
            ),
        ])
    );
    assert_eq!(c.settings.image_preview, ImagePreview::Auto);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Private);
    assert_eq!(c.time_format, TimeFormat::default());
    assert_eq!(c.settings.room_sort, RoomSort::default());
    assert_eq!(c.room_list_layout.network, NetworkPlacement::Inline);
    // The log and `:messages` show them with the file.
    assert_eq!(
        c.warnings[1].to_string(),
        "config.toml:5:17: ui.read_receipts: unknown value \"x\", using \"private\""
    );
}

/// The room sort and the warnings of `ui.room_list.sort = <value>`, which
/// are all at the value's start, line 4, column 22.
fn sort(value: &str) -> (RoomSort, Vec<String>) {
    let (c, warnings) = warned(&format!(
        "{MINIMAL}[ui]\nroom_list = {{ sort = {value:?} }}\n"
    ));
    let messages = warnings
        .into_iter()
        .map(|(line, column, message)| {
            assert_eq!((line, column), (4, 22), "{message}");
            message
        })
        .collect();
    (c.settings.room_sort, messages)
}

#[test]
fn room_list_sort_is_a_list_of_keys() {
    use SortKey::{Mentions, Name, Notify, Read, Recent, Unread};
    let list = |keys: &[SortKey]| RoomSort::new(keys.to_vec()).unwrap();
    let unknown = |key: &str| {
        format!(
            "ui.room_list.sort: unknown value {key:?}; expected one of \"mentions\", \
             \"notify\", \"unread\", \"recent\", \"read\", \"name\" (ignored)"
        )
    };
    for (value, key) in [
        ("mentions", Mentions),
        ("notify", Notify),
        ("unread", Unread),
        ("recent", Recent),
        ("read", Read),
        ("name", Name),
    ] {
        assert_eq!(sort(value), (RoomSort::one(key), vec![]), "{value}");
    }
    for value in [
        "mentions, unread, read",
        "mentions,unread,read",
        " mentions ,unread,  read ",
    ] {
        assert_eq!(
            sort(value),
            (list(&[Mentions, Unread, Read]), vec![]),
            "{value}"
        );
    }
    assert_eq!(
        sort("mentions, notify, unread, read"),
        (list(&[Mentions, Notify, Unread, Read]), vec![])
    );
    // An unknown key is skipped; the others apply.
    assert_eq!(
        sort("Mentions, unread"),
        (list(&[Unread]), vec![unknown("Mentions")])
    );
    assert_eq!(
        sort("mentions, size, read"),
        (list(&[Mentions, Read]), vec![unknown("size")])
    );
    assert_eq!(
        sort("size, age"),
        (RoomSort::default(), vec![unknown("size"), unknown("age")])
    );
    // A list that cannot be used: the default.
    for value in ["", " "] {
        assert_eq!(
            sort(value),
            (
                RoomSort::default(),
                vec!["ui.room_list.sort: names no sort key (ignored)".to_owned()]
            ),
            "{value:?}"
        );
    }
    for value in ["mentions,,read", "mentions, read,", ",read"] {
        assert_eq!(
            sort(value),
            (
                RoomSort::default(),
                vec![format!(
                    "ui.room_list.sort: empty sort key in {value:?} (ignored)"
                )]
            ),
            "{value:?}"
        );
    }
    let twice = |key: &str| format!("ui.room_list.sort: {key:?} is listed twice (ignored)");
    assert_eq!(
        sort("unread, read, unread"),
        (RoomSort::default(), vec![twice("unread")])
    );
    assert_eq!(
        sort("size, read, read"),
        (RoomSort::default(), vec![unknown("size"), twice("read")])
    );
}

#[test]
fn a_binding_with_an_unknown_name_is_ignored_and_the_default_stays() {
    let (c, warnings) = warned(
        r#"[account]
user_id = "@alice:example.org"
[keymap.timeline]
"x" = "message.dlete"
"dd" = "message.dlete"
"D" = ["message.delete", "message.future"]
"]n" = { action = "room.open", sort = "name" }
"]m" = { action = "room.open", filter = "dm" }
"+" = { action = "message.react", emoji = "nonsense" }
"<F13>" = "app.quit"
"t" = "message.delete"
"#,
    );
    assert_eq!(
        warnings,
        expected(&[
            (
                4,
                7,
                "keymap.timeline.x: unknown action \"message.dlete\" (ignored)"
            ),
            (
                5,
                8,
                "keymap.timeline.dd: unknown action \"message.dlete\" (ignored)"
            ),
            (
                6,
                26,
                "keymap.timeline.D: unknown action \"message.future\" (ignored)"
            ),
            (
                7,
                32,
                "keymap.timeline.\"]n\": room.open: unknown argument \"sort\" (ignored)"
            ),
            (
                8,
                32,
                "keymap.timeline.\"]m\": room.open: unknown \"filter\" value \"dm\"; expected one \
                 of \"all\", \"unread\", \"notify\", \"mentions\" (ignored)"
            ),
            (
                9,
                35,
                "keymap.timeline.\"+\": message.react: unknown emoji \"nonsense\"; expected an \
                 emoji or a shortcode, such as \"👍\", \"thumbsup\" or \":+1:\" (ignored)"
            ),
            (
                10,
                1,
                "keymap.timeline: invalid key \"<F13>\": unknown key <F13> (ignored)"
            ),
        ])
    );
    let timeline = |keys: &str| lookup(&c, InputContext::Timeline, keys);
    assert_eq!(timeline("x"), None);
    assert_eq!(timeline("dd"), Some(vec!["message.delete"]));
    // The array is ignored whole.
    assert_eq!(timeline("D"), None);
    assert_eq!(timeline("]n"), None);
    assert_eq!(timeline("+"), Some(vec!["message.react"]));
    assert_eq!(timeline("t"), Some(vec!["message.delete"]));
}

#[test]
fn arguments_of_other_toml_types_reach_the_action() {
    let (_, warnings) = warned(
        r#"[account]
user_id = "@alice:example.org"
[keymap.timeline]
"a" = { action = "message.future", ids = [1, 2] }
"b" = { action = "room.open", ratio = 0.5 }
"c" = { action = "room.open", since = 1979-05-27T07:32:00Z }
"d" = { action = "room.open", sort = { by = "name" } }
"#,
    );
    assert_eq!(
        warnings,
        expected(&[
            (
                4,
                7,
                "keymap.timeline.a: unknown action \"message.future\" (ignored)"
            ),
            (
                5,
                31,
                "keymap.timeline.b: room.open: unknown argument \"ratio\" (ignored)"
            ),
            (
                6,
                31,
                "keymap.timeline.c: room.open: unknown argument \"since\" (ignored)"
            ),
            (
                7,
                31,
                "keymap.timeline.d: room.open: unknown argument \"sort\" (ignored)"
            ),
        ])
    );
}

#[test]
fn the_first_problem_of_a_binding_decides() {
    // The unknown argument comes before the conflict.
    assert_eq!(
        warning(&format!(
            "{MINIMAL}[keymap.normal]\n\"x\" = {{ action = \"room.open\", sort = \"name\", pick = \"next\", query = \"x\" }}\n"
        )),
        (
            4,
            31,
            "keymap.normal.x: room.open: unknown argument \"sort\" (ignored)".to_owned()
        )
    );
    // The conflict comes before the values.
    assert_eq!(
        err_at(&format!(
            "{MINIMAL}[keymap.normal]\n\"x\" = {{ action = \"room.open\", pick = \"next\", query = \"x\", filter = \"dm\" }}\n"
        )),
        (
            4,
            46,
            "keymap.normal.x: room.open: \"pick\" and \"query\" cannot be used together".to_owned()
        )
    );
}

#[test]
fn an_unknown_leader_falls_back_to_space() {
    let text =
        |bindings: &str| format!("{MINIMAL}[ui]\nleader = \"<F13>\"\n[keymap.normal]\n{bindings}");
    let (c, warnings) = warned(&text(
        "\"<leader>x\" = \"app.quit\"\n\"<Space>x\" = \"nop\"\n",
    ));
    assert_eq!(
        warnings,
        expected(&[
            (
                4,
                10,
                "ui.leader: invalid key \"<F13>\": unknown key <F13> (ignored)"
            ),
            (
                7,
                1,
                "keymap.normal: \"<Space>x\" is the same key sequence as \"<leader>x\" on line 6, \
                 since ui.leader fell back to \"<Space>\" (ignored)"
            ),
        ])
    );
    assert_eq!(
        lookup(&c, InputContext::Timeline, "<Space>x"),
        Some(vec!["app.quit"])
    );
    assert_eq!(
        lookup(&c, InputContext::Timeline, "<Space>e"),
        Some(vec!["sidebar.toggle"])
    );
    // Spellings that are the same key sequence in any version stay an error.
    let (line, msg) = err(&text("\"<C-a>\" = \"nop\"\n\"<C-A>\" = \"app.quit\"\n"));
    assert_eq!(line, 7);
    assert!(msg.ends_with("on line 6"), "{msg}");
    let (line, msg) = err(&text(
        "\"<leader>x\" = \"nop\"\n\"<Leader>x\" = \"app.quit\"\n",
    ));
    assert_eq!(line, 7);
    assert!(msg.ends_with("on line 6"), "{msg}");
    // Both spellings may write `<leader>`: only the fallback makes these one.
    let (_, warnings) = warned(&text(
        "\"<leader>x<Space>\" = \"app.quit\"\n\"<Space>x<leader>\" = \"nop\"\n",
    ));
    assert_eq!(
        warnings,
        expected(&[
            (
                4,
                10,
                "ui.leader: invalid key \"<F13>\": unknown key <F13> (ignored)"
            ),
            (
                7,
                1,
                "keymap.normal: \"<Space>x<leader>\" is the same key sequence as \
                 \"<leader>x<Space>\" on line 6, since ui.leader fell back to \"<Space>\" (ignored)"
            ),
        ])
    );
}

#[test]
fn a_ctrl_i_binding_that_a_tab_binding_duplicates_is_ignored_with_a_warning() {
    let picker = |bindings: &str| format!("{MINIMAL}[keymap.picker]\n{bindings}");
    // Whichever comes first, the `<Tab>` binding stays: it was the only one
    // that could fire.
    for (bindings, line, message) in [
        (
            "\"<Tab>\" = \"picker.prev\"\n\"<C-i>\" = \"picker.cancel\"\n",
            5,
            "keymap.picker: \"<C-i>\" is the same key sequence as \"<Tab>\" on line 4, \
             since <C-i> is another name for <Tab> (ignored)",
        ),
        (
            "\"<C-i>\" = \"picker.cancel\"\n\"<Tab>\" = \"picker.prev\"\n",
            4,
            "keymap.picker: \"<C-i>\" is the same key sequence as \"<Tab>\" on line 5, \
             since <C-i> is another name for <Tab> (ignored)",
        ),
    ] {
        let (c, warnings) = warned(&picker(bindings));
        assert_eq!(warnings, expected(&[(line, 1, message)]), "{bindings}");
        assert_eq!(
            lookup(&c, InputContext::Picker, "<Tab>"),
            Some(vec!["picker.prev"]),
            "{bindings}"
        );
    }
    // Inside a sequence and with alt too.
    let (c, warnings) = warned(&format!(
        "{MINIMAL}[keymap.normal]\n\"<C-w><Tab>\" = \"focus.timeline\"\n\
         \"<C-w><C-i>\" = \"app.quit\"\n\"<C-A-i>\" = \"app.quit\"\n\"<A-Tab>\" = \"sidebar.toggle\"\n"
    ));
    let lines: Vec<usize> = warnings.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, vec![5, 6], "{warnings:#?}");
    assert_eq!(
        lookup(&c, InputContext::Timeline, "<C-w><Tab>"),
        Some(vec!["focus.timeline"])
    );
    assert_eq!(
        lookup(&c, InputContext::Timeline, "<A-Tab>"),
        Some(vec!["sidebar.toggle"])
    );
    // An earlier `<C-i>` binding ignored for its action gets no second
    // warning, and one ignored for the alias leaves an ignored `<Tab>`
    // binding to the default.
    let (c, warnings) = warned(&picker(
        "\"<C-i>\" = \"picker.future\"\n\"<Tab>\" = \"picker.prev\"\n",
    ));
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].2.contains("unknown action"), "{warnings:#?}");
    assert_eq!(
        lookup(&c, InputContext::Picker, "<Tab>"),
        Some(vec!["picker.prev"])
    );
    let (c, warnings) = warned(&picker(
        "\"<C-i>\" = \"picker.cancel\"\n\"<Tab>\" = \"picker.future\"\n",
    ));
    let lines: Vec<usize> = warnings.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, vec![4, 5], "{warnings:#?}");
    assert_eq!(
        lookup(&c, InputContext::Picker, "<Tab>"),
        Some(vec!["picker.next"])
    );
    // Every other duplicate stays an error, a second spelling of `<C-i>`
    // too.
    let (line, msg) = err(&picker(
        "\"<Tab>\" = \"picker.prev\"\n\"<C-i>\" = \"picker.cancel\"\n\"<C-I>\" = \"nop\"\n",
    ));
    assert_eq!(line, 6);
    assert!(
        msg.ends_with("\"<C-I>\" is the same key sequence as \"<C-i>\" on line 5"),
        "{msg}"
    );
    let (line, msg) = err(&picker(
        "\"<Tab>\" = \"picker.prev\"\n\"<tab>\" = \"nop\"\n",
    ));
    assert_eq!(line, 5);
    assert!(msg.ends_with("on line 4"), "{msg}");
}

#[test]
fn wrong_types_and_malformed_values_stay_errors() {
    for (text, line, column, message) in [
        (
            "[keymap.normal]\n\"x\" = { action = \"room.open\", filter = 3.5 }\n",
            4,
            31,
            "keymap.normal.x: room.open: \"filter\" must be a string",
        ),
        (
            "[keymap.normal]\n\"x\" = { action = \"message.react\", emoji = \"\" }\n",
            4,
            35,
            "keymap.normal.x: message.react: \"emoji\" must not be empty",
        ),
        (
            "[keymap.normal]\n\"<C-leader>x\" = \"nop\"\n",
            4,
            1,
            "keymap.normal: invalid key \"<C-leader>x\": <leader> takes no modifiers",
        ),
        (
            "[ui]\nleader = \"<C-leader>\"\n",
            4,
            10,
            "ui.leader: invalid key \"<C-leader>\": <leader> takes no modifiers",
        ),
        (
            "[ui]\nleader = \"<leader>\"\n",
            4,
            10,
            "ui.leader: invalid key \"<leader>\": <leader> cannot be used in the leader itself",
        ),
        (
            "[keymap.normal]\n\"<C-x<CR>\" = \"nop\"\n",
            4,
            1,
            "keymap.normal: invalid key \"<C-x<CR>\": \"<\" at byte 0 is not closed by \">\"; \
             write <lt> for a literal \"<\"",
        ),
        (
            "[ui]\nleader = \"<C-x<CR>\"\n",
            4,
            10,
            "ui.leader: invalid key \"<C-x<CR>\": \"<\" at byte 0 is not closed by \">\"; \
             write <lt> for a literal \"<\"",
        ),
        (
            "[keymap.normal]\n\"<>\" = \"nop\"\n",
            4,
            1,
            "keymap.normal: invalid key \"<>\": \"<\" at byte 0 has no key before \">\"",
        ),
        (
            "[ui]\nleader = \"<>\"\n",
            4,
            10,
            "ui.leader: invalid key \"<>\": \"<\" at byte 0 has no key before \">\"",
        ),
    ] {
        assert_eq!(
            err_at(&format!("{MINIMAL}{text}")),
            (line, column, message.to_owned()),
            "{text}"
        );
    }
    // A syntax error, and a value of the wrong type for a known key.
    for (text, line) in [
        ("[ui\n", 3),
        ("[ui]\nread_receipts = 1\n", 4),
        ("[ui]\nroom_list = { sort = true }\n", 4),
        ("[keymap.normal]\n\"x\" = 3\n", 4),
        ("[keymap.normal]\n\"x\" = { action = 3 }\n", 4),
    ] {
        assert_eq!(err(&format!("{MINIMAL}{text}")).0, line, "{text}");
    }
}

#[test]
fn a_missing_key_is_printed_with_the_warnings_before_it() {
    let printed = |text: &str| {
        parse(text, Path::new("config.toml"), Path::new("/home/alice"))
            .unwrap_err()
            .to_string()
    };
    // A missing `account` is about the file, not about the first table.
    assert_eq!(
        printed("[acount]\nuser_id = \"@alice:example.org\"\n"),
        "config.toml:1:1: missing field `account`\n\
         config.toml:1:2: acount: unknown table (ignored)"
    );
    assert_eq!(
        printed("[ui]\ntimeoutlen = 300\n"),
        "config.toml:1:1: missing field `account`"
    );
    assert_eq!(
        printed("[account]\nuser-id = \"@alice:example.org\"\n[ui]\nfont = \"Iosevka\"\n"),
        "config.toml:1:1: account: missing field `user_id`\n\
         config.toml:2:1: account.user-id: unknown key (ignored)\n\
         config.toml:4:1: ui.font: unknown key (ignored)"
    );
}

#[test]
fn every_key_and_table_loads_without_warnings() {
    use std::fmt::Write as _;

    // Every key of every DTO struct, none at its default.
    let mut text = r#"
[account]
user_id = "@alice:example.org"
homeserver = "https://matrix.example.org"

[ui]
leader = ","
timeoutlen = 300
room_list = { visible = false, sort = "mentions, read", width = 40, network = "below" }
time_format = "%Y-%m-%d %H:%M"
image_preview = "kitty"
read_receipts = "private"

[files]
download_dir = "/tmp/dl"
opener = ["wslview"]
"#
    .to_owned();
    // A binding in every table, under a header written from its name.
    let keys = |n: usize| format!("<F{}>", n + 1);
    for (n, id) in TableId::ALL.iter().enumerate() {
        _ = write!(
            text,
            "\n[keymap.{}]\n\"{}\" = \"app.quit\"\n",
            id.config_name(),
            keys(n)
        );
    }
    let c = ok(&text);

    assert_eq!(c.homeserver.as_deref(), Some("https://matrix.example.org"));
    let comma_e = [Key::char(','), Key::char('e')];
    let sidebar = c.keymap.effective(InputContext::Timeline).get(&comma_e);
    assert_eq!(sidebar, Some(&Binding::single(Action::SidebarToggle)));
    assert_eq!(c.settings.timeoutlen_ms, 300);
    assert!(!c.settings.room_list_visible);
    assert_eq!(
        c.settings.room_sort,
        RoomSort::new(vec![SortKey::Mentions, SortKey::Read]).unwrap()
    );
    assert_eq!(
        c.room_list_layout,
        RoomListLayout {
            width: RoomListWidth::new(40).unwrap(),
            network: NetworkPlacement::Below,
        }
    );
    assert_eq!(
        c.time_format,
        TimeFormat::parse("%Y-%m-%d %H:%M".to_owned()).unwrap()
    );
    assert_eq!(
        c.settings.image_preview,
        ImagePreview::Forced(ImageProtocol::Kitty)
    );
    assert_eq!(c.settings.read_receipts, ReceiptKind::Private);
    assert_eq!(c.download_dir, PathBuf::from("/tmp/dl"));
    assert_eq!(c.opener, vec!["wslview".to_owned()]);
    for (n, &id) in TableId::ALL.iter().enumerate() {
        let context = match id {
            TableId::Normal | TableId::RoomList => InputContext::RoomList,
            TableId::Timeline => InputContext::Timeline,
            TableId::Thread => InputContext::Thread,
            TableId::ComposerNormal => InputContext::ComposerNormal,
            TableId::ComposerVisual => InputContext::ComposerVisual,
            TableId::ComposerInsert => InputContext::ComposerInsert,
            TableId::ComposerCompletion => InputContext::ComposerCompletion,
            TableId::MessageView => InputContext::MessageView,
            TableId::Picker => InputContext::Picker,
            TableId::CommandLine => InputContext::CommandLine,
        };
        assert_eq!(
            lookup(&c, context, &keys(n)),
            Some(vec!["app.quit"]),
            "{id:?}"
        );
    }
}

#[test]
fn the_complete_example_of_the_documentation_loads() {
    const DOCUMENT: &str = include_str!("../../../../docs/configuration.md");
    let example = DOCUMENT
        .split_once("Every setting with its default")
        .and_then(|(_, after)| after.split_once("```toml\n"))
        .and_then(|(_, block)| block.split_once("```"))
        .map(|(example, _)| example)
        .expect("docs/configuration.md has its complete example");
    let c = ok(example);
    assert_eq!(
        lookup(&c, InputContext::ComposerInsert, "<C-s>"),
        Some(vec!["composer.send"])
    );
}

#[test]
fn default_path_follows_xdg() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    assert_eq!(
        default_path(&env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/alice")])),
        Some(PathBuf::from("/x/mxt/config.toml"))
    );
    let fallback = Some(PathBuf::from("/home/alice/.config/mxt/config.toml"));
    assert_eq!(
        default_path(&env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/alice")])),
        fallback
    );
    assert_eq!(
        default_path(&env(&[("XDG_CONFIG_HOME", "x"), ("HOME", "/home/alice")])),
        fallback
    );
    assert_eq!(default_path(&env(&[])), None);
    assert_eq!(default_path(&env(&[("HOME", "home/alice")])), None);

    // A value that is not UTF-8 is used as it is.
    let config = OsString::from_vec(b"/x\xff".to_vec());
    assert_eq!(
        default_path(&|name: &str| (name == "XDG_CONFIG_HOME").then(|| config.clone())),
        Some(PathBuf::from(config.clone()).join("mxt/config.toml"))
    );
}
