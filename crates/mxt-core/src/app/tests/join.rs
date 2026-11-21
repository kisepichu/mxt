//! `:join` (design, "Joining rooms and direct messages").

use super::timeline::status;
use super::*;
use crate::action::ArgValue;
use crate::msg::JoinedHow;
use crate::room::Invite;

const BOB: &str = "@bob:example.org";
const NEW: &str = "!new:example.org";
const QUESTION: &str = "Create a direct message with @bob:example.org? (y/n)";

fn join(target: &str) -> Effect {
    Effect::Matrix(MatrixEffect::Join {
        target: target.into(),
    })
}
fn create_dm() -> Effect {
    Effect::Matrix(MatrixEffect::CreateDm {
        user: UserId::new(BOB),
    })
}
fn subscribe(room: &str) -> Effect {
    Effect::Matrix(MatrixEffect::SubscribeTimeline(id(room)))
}
fn joined(target: &str, room: &str, how: JoinedHow) -> Msg {
    Msg::Joined {
        target: target.into(),
        room: id(room),
        how,
    }
}
fn no_dm(target: &str) -> Msg {
    Msg::NoDirectMessage {
        target: target.into(),
        user: UserId::new(BOB),
    }
}
fn with_invites(invites: &[(&str, &str)]) -> App {
    let mut app = app();
    app.update(Msg::Invites(
        invites
            .iter()
            .map(|&(room, name)| Invite {
                room: id(room),
                name: name.into(),
            })
            .collect(),
    ));
    app
}
fn error(app: &App) -> Option<&str> {
    app.status()
        .filter(|status| status.kind == StatusKind::Error)
        .map(|status| status.text.as_str())
}

#[test]
fn each_target_form_goes_to_the_adapter_as_typed() {
    for target in [
        "#mxt:example.org",
        NEW,
        // Newer room versions have IDs without a server name.
        "!abc",
        BOB,
        "https://matrix.to/#/%23mxt%3Aexample.org?via=example.org",
        "https://matrix.to/#/!a:example.org/$event",
    ] {
        let mut app = app();
        assert_eq!(
            keys(&mut app, &format!(":join {target}<CR>")),
            vec![join(target)],
            "{target}"
        );
        assert_eq!(status(&app), Some("Joining…"));
    }
}

#[test]
fn an_invite_name_joins_the_invites_room() {
    let mut app = with_invites(&[("!book:example.org", "Book club"), (NEW, "Alice")]);
    assert_eq!(
        keys(&mut app, ":join book<CR>"),
        vec![join("!book:example.org")]
    );
    // A name with one of the forms' first characters but not the form.
    let mut app = with_invites(&[("!hash:example.org", "#general")]);
    assert_eq!(
        keys(&mut app, ":join #general<CR>"),
        vec![join("!hash:example.org")]
    );
}

#[test]
fn a_name_no_invite_matches_is_an_error() {
    let mut app = with_invites(&[("!book:example.org", "Book club")]);
    assert_eq!(keys(&mut app, ":join chess<CR>"), vec![]);
    assert_eq!(error(&app), Some("No pending invite matches \"chess\""));
    let mut app = super::app();
    assert_eq!(keys(&mut app, ":join Alice Smith<CR>"), vec![]);
    assert_eq!(
        error(&app),
        Some("No pending invite matches \"Alice Smith\"")
    );
}

#[test]
fn two_invites_with_one_name_are_joined_by_their_room_ids() {
    let mut app = with_invites(&[
        ("!one:example.org", "Alice"),
        ("!two:example.org", "Alice"),
        ("!book:example.org", "Book club"),
    ]);
    assert_eq!(keys(&mut app, ":join Alice<CR>"), vec![]);
    assert_eq!(
        error(&app),
        Some("More than one pending invite is named \"Alice\"; <Tab> completes their room IDs")
    );
    keys(&mut app, ":join Ali");
    for shown in ["join !one:example.org", "join !two:example.org", "join Ali"] {
        keys(&mut app, "<Tab>");
        assert_eq!(line(&app), Some((LineKind::Command, shown)));
    }
    keys(&mut app, "<Tab>");
    assert_eq!(keys(&mut app, "<CR>"), vec![join("!one:example.org")]);
}

#[test]
fn a_room_id_of_a_listed_room_opens_it_without_a_request() {
    let mut app = app();
    assert_eq!(
        keys(&mut app, ":join !b:example.org<CR>"),
        vec![subscribe("!b:example.org")]
    );
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert_eq!(status(&app), None);
}

#[test]
fn the_answered_room_opens_when_listed_or_once_it_is() {
    let mut app = app();
    keys(&mut app, ":join #b:example.org<CR>");
    assert_eq!(
        app.update(joined(
            "#b:example.org",
            "!b:example.org",
            JoinedHow::Joined
        )),
        vec![subscribe("!b:example.org")]
    );
    assert_eq!(app.focus(), Focus::Timeline);
    assert_eq!(status(&app), Some("Joined b:example.org"));

    let mut app = super::app();
    keys(&mut app, &format!(":join {NEW}<CR>"));
    assert_eq!(app.update(joined(NEW, NEW, JoinedHow::Joined)), vec![]);
    assert_eq!(app.open_room(), None);
    // Another room's update does not open it.
    assert_eq!(
        app.update(Msg::RoomUpserted(room("!a:example.org", 50, 0))),
        vec![]
    );
    assert_eq!(
        app.update(Msg::RoomUpserted(room(NEW, 60, 0))),
        vec![subscribe(NEW)]
    );
    assert_eq!(status(&app), Some("Joined new:example.org"));
    // The wait has ended.
    keys(&mut app, "<C-w>h");
    app.update(Msg::RoomUpserted(room(NEW, 70, 0)));
    assert_eq!(app.focus(), Focus::RoomList);
}

#[test]
fn already_clears_the_joining_status_and_created_says_so() {
    let mut app = app();
    keys(&mut app, ":join #c:example.org<CR>");
    app.update(joined(
        "#c:example.org",
        "!c:example.org",
        JoinedHow::Already,
    ));
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
    assert_eq!(status(&app), None);

    let mut app = super::app();
    keys(&mut app, &format!(":join {BOB}<CR>"));
    app.update(no_dm(BOB));
    keys(&mut app, "y");
    assert_eq!(status(&app), Some("Creating a direct message…"));
    app.update(joined(BOB, "!a:example.org", JoinedHow::Created));
    assert_eq!(app.open_room(), Some(&id("!a:example.org")));
    assert_eq!(status(&app), Some("Created a direct message"));
}

#[test]
fn a_key_or_another_join_ends_the_wait() {
    let mut app = app();
    keys(&mut app, &format!(":join {NEW}<CR>"));
    keys(&mut app, "j");
    assert_eq!(
        app.update(joined(NEW, "!b:example.org", JoinedHow::Joined)),
        vec![]
    );
    assert_eq!(app.open_room(), None);

    // The answer came before the room was listed; the key ends that wait too.
    let mut app = super::app();
    keys(&mut app, &format!(":join {NEW}<CR>"));
    app.update(joined(NEW, NEW, JoinedHow::Joined));
    paste(&mut app, "x", None);
    assert_eq!(app.update(Msg::RoomUpserted(room(NEW, 60, 0))), vec![]);
    assert_eq!(app.open_room(), None);

    // An answer to an earlier `:join` changes nothing.
    let mut app = super::app();
    keys(&mut app, ":join #b:example.org<CR>:join #c:example.org<CR>");
    assert_eq!(
        app.update(joined(
            "#b:example.org",
            "!b:example.org",
            JoinedHow::Joined
        )),
        vec![]
    );
    assert_eq!(status(&app), Some("Joining…"));
    assert_eq!(
        app.update(joined(
            "#c:example.org",
            "!c:example.org",
            JoinedHow::Joined
        )),
        vec![subscribe("!c:example.org")]
    );
}

#[test]
fn no_direct_message_asks_before_one_is_created() {
    let mut app = app();
    keys(&mut app, &format!(":join {BOB}<CR>"));
    assert_eq!(app.update(no_dm(BOB)), vec![]);
    assert_eq!(app.confirmation().as_deref(), Some(QUESTION));
    assert_eq!(app.mode_label(None), "CONFIRM");
    assert_eq!(keys(&mut app, "y"), vec![create_dm()]);
    assert_eq!(app.confirmation(), None);

    for cancel in ["n", "<Esc>"] {
        let mut app = super::app();
        keys(&mut app, &format!(":join {BOB}<CR>"));
        app.update(no_dm(BOB));
        assert_eq!(keys(&mut app, cancel), vec![], "{cancel}");
        assert_eq!(app.confirmation(), None);
        // The answer ended the wait.
        assert_eq!(no_timers(keys(&mut app, "y")), vec![]);
    }

    // A link to the user is the target the answer carries.
    let link = "https://matrix.to/#/@bob:example.org";
    let mut app = super::app();
    keys(&mut app, &format!(":join {link}<CR>"));
    app.update(no_dm(link));
    assert_eq!(keys(&mut app, "y"), vec![create_dm()]);
}

#[test]
fn the_question_is_not_in_the_status_history() {
    let mut app = app();
    keys(&mut app, &format!(":join {BOB}<CR>"));
    app.update(no_dm(BOB));
    keys(&mut app, "n");
    assert_eq!(messages(&mut app)[2..], ["Joining…"]);
}

#[test]
fn no_direct_message_after_a_key_only_says_so() {
    let mut app = app();
    keys(&mut app, &format!(":join {BOB}<CR>j"));
    app.update(no_dm(BOB));
    assert_eq!(app.confirmation(), None);
    assert_eq!(
        status(&app),
        Some("No direct message found; :join again to create one")
    );
    // A `y` typed then answers nothing.
    assert_eq!(no_timers(keys(&mut app, "y")), vec![]);

    // While another `:join` is waited for: nothing.
    let mut app = super::app();
    keys(
        &mut app,
        &format!(":join {BOB}<CR>:join #b:example.org<CR>"),
    );
    app.update(no_dm(BOB));
    assert_eq!(app.confirmation(), None);
    assert_eq!(status(&app), Some("Joining…"));
}

#[test]
fn join_completes_its_name_and_invite_names() {
    // In the adapter's order, by name.
    let mut app = with_invites(&[(NEW, "Bob"), ("!book:example.org", "Book club")]);
    keys(&mut app, ":j<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "join ")));
    for shown in ["join Bob", "join Book club", "join "] {
        keys(&mut app, "<Tab>");
        assert_eq!(line(&app), Some((LineKind::Command, shown)));
    }
    keys(&mut app, "<C-u>join cl<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "join Book club")));
    // Without invites there is nothing to complete.
    let mut app = super::app();
    keys(&mut app, ":join b<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "join b")));
}

#[test]
fn a_binding_joins_its_target() {
    let target = ArgValue::String("#mxt:example.org".into());
    let action = Action::from_spec("room.join", &[("target".into(), target)]).unwrap();
    let mut app = app_with(vec![(TableId::Normal, seq("J"), Binding::single(action))]);
    assert_eq!(keys(&mut app, "J"), vec![join("#mxt:example.org")]);
    assert!(
        keymap_text(&app.keymap)
            .lines()
            .any(|line| line == "J -> room.join (target = \"#mxt:example.org\")")
    );
}
