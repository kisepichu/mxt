use super::*;
use mxt_core::action::Action;
use mxt_core::keymap::InputContext;
use mxt_core::keys::KeySeq;
use mxt_core::rooms::RoomSort;
use mxt_core::settings::{ImagePreview, ReceiptKind};
use mxt_tui::NetworkPlacement;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

fn ok(text: &str) -> Config {
    parse(text, Path::new("config.toml"), Path::new("/home/alice"))
        .unwrap_or_else(|e| panic!("{e}"))
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
    assert_eq!(c.settings.room_sort, RoomSort::Recent);
    assert!(c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Auto);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Public);
    assert_eq!(c.time_format, TimeFormat::default());
    assert_eq!(
        c.room_list_layout,
        RoomListLayout {
            width: 30,
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
"]u" = { action = "room.open", filter = "unread", pick = "next" }
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]

[keymap.timeline]
"dd" = "nop"
"D" = "message.delete"

[keymap.composer.insert]
"<C-s>" = "composer.send"
"#);
    assert_eq!(
        lookup(&c, InputContext::Timeline, "]u"),
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
fn settings_are_read() {
    let c = ok(&format!(
        "{MINIMAL}[ui]\ntimeoutlen = 300\nread_receipts = \"private\"\nroom_list = {{ visible = false, sort = \"name\", width = 40, network = \"below\" }}\nimage_preview = \"off\"\n[files]\ndownload_dir = \"/tmp/dl\"\nopener = [\"wslview\"]\n"
    ));
    assert_eq!(c.settings.timeoutlen_ms, 300);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Private);
    assert_eq!(c.settings.room_sort, RoomSort::Name);
    assert!(!c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Off);
    assert_eq!(
        c.room_list_layout,
        RoomListLayout {
            width: 40,
            network: NetworkPlacement::Below,
        }
    );
    assert_eq!(c.download_dir, PathBuf::from("/tmp/dl"));
    assert_eq!(c.opener, vec!["wslview".to_string()]);
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
    let (line, msg) = err(&format!(
        "{MINIMAL}[ui]\ntimeoutlen = 300\ntime_format = \"%H:%K\"\n"
    ));
    assert_eq!(line, 5);
    assert!(msg.starts_with("ui.time_format: "), "{msg}");
    let (line, msg) = err(&format!("{MINIMAL}[ui]\ntime_format = \"\"\n"));
    assert_eq!(line, 4);
    assert!(msg.starts_with("ui.time_format: "), "{msg}");
}

#[test]
fn the_room_list_width_is_from_12_to_80() {
    for width in [12, 80] {
        let c = ok(&format!(
            "{MINIMAL}[ui]\nroom_list = {{ width = {width} }}\n"
        ));
        assert_eq!(c.room_list_layout.width, width);
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
        // An unknown argument.
        ("filter = \"unread\"\npik = \"next\"\n", 6),
        // A wrong type.
        ("filter = 3\n", 5),
        // An invalid value.
        ("pick = \"next\"\nfilter = \"unred\"\n", 6),
        // A conflict: the later of the two in the file.
        ("pick = \"next\"\nfilter = \"all\"\nquery = \"x\"\n", 7),
        ("query = \"x\"\npick = \"next\"\n", 6),
    ] {
        assert_eq!(err(&header(args)).0, line, "{args}");
    }
    // Errors about the action as a whole stay at the binding.
    let (line, msg) = err(&format!(
        "{MINIMAL}[keymap.normal.x]\naction = \"file.upload\"\n"
    ));
    assert_eq!(line, 3);
    assert!(msg.contains("path"), "{msg}");
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
fn errors_name_the_line() {
    let (line, msg) = err(&format!("{MINIMAL}[ui]\ntimeoutln = 5\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("timeoutln"), "{msg}");

    let (line, msg) = err(&format!(
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

    let (line, _) = err(&format!(
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

    let (line, msg) = err(&format!(
        "{MINIMAL}[ui]\nroom_list = {{ sort = \"size\" }}\n"
    ));
    assert_eq!(line, 4);
    assert!(
        msg.contains("ui.room_list.sort") && msg.contains("recent"),
        "{msg}"
    );

    let (line, column, msg) = err_at(&format!(
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
