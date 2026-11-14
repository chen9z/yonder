//! The app as a person uses it, headless: adding a computer (a test one:
//! SSH here, an isolated herdr-core behind it, never the user's), then
//! working in it.

use herdc_testkit::computer::{self, Options};
use herdc_testkit::Session;
use nsh_core::prelude::{point, px, size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless, MemoryStorage};
use std::time::Duration;

const SLOW: Duration = Duration::from_secs(30);

fn hc_terminals(session: &Session) -> Vec<serde_json::Value> {
    let out = session.hc(&["--json", "ls"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    v.as_array().or(v["terminals"].as_array()).cloned().unwrap_or_default()
}

/// Waits for the tab strip to show tab `n` (the bar under it says only
/// where the space is, then).
fn shows_tab(app: &mut Headless, n: usize) {
    let tab = format!("role:tab[{n}]");
    app.wait_until(&format!("tab {n} shown"), SLOW, |screen| screen.selected(&tab) == Ok(true)).unwrap();
}

/// Picks choice `id` from the app's sheet of choices, once it shows.
fn pick(app: &mut Headless, id: &str) {
    let row = format!("id:pick-{id}");
    app.wait_for_within(&row, SLOW).unwrap();
    app.press(&row).unwrap();
}

/// Answers the next dialog by pressing `button`, checking it says `says`.
fn answer(app: &mut Headless, button: &'static str, says: &'static str) {
    app.on_dialog(move |d| {
        let all = format!("{:?} {:?} {:?}", d.title, d.message, d.buttons);
        assert!(all.contains(says), "a dialog saying {says:?}: {all}");
        DialogAnswer::Press(button.to_owned())
    });
}

#[test]
fn adds_a_computer_then_works_there() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = Session::new();
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();

    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    let storage = MemoryStorage::new(Vec::<(String, String)>::new());
    config.storage = Some(storage.clone());
    // The links the app opens, recorded rather than opened.
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let opened = opened.clone();
        config.host.handle_open_url(move |request| {
            opened.lock().unwrap().push(request.url().to_owned());
            request.handled();
        });
    }
    let mut app = Headless::start(config).unwrap();
    app.wait_for("text:Connect").unwrap();

    // The way in: the host key is shown to compare, then the password goes.
    app.type_text("id:display name", "Studio").unwrap();
    app.type_text("id:computer", "127.0.0.1").unwrap();
    app.type_text("id:user", "me").unwrap();
    app.clear("id:port").unwrap();
    app.type_text("id:port", &served.port.to_string()).unwrap();
    app.type_text("id:password", "open sesame").unwrap();
    let fingerprint = Box::leak(served.fingerprint.clone().into_boxed_str());
    answer(&mut app, "Trust", fingerprint);
    app.press("text:Connect").unwrap();
    app.wait_for_within("label:Spaces", SLOW).unwrap();
    app.press("label:Spaces").unwrap();
    app.wait_for_within("text:No spaces yet", SLOW).unwrap();
    // Nothing is left on the computer: the password is kept (sealed on a
    // phone; off one, as it is).
    assert!(!session.path(".ssh/authorized_keys").exists(), "a key was left");
    let stored = storage.entries();
    assert!(format!("{stored:?}").contains("open sesame"), "{stored:?}");

    // A space starts a terminal on the computer.
    app.press("role:button[New space]").unwrap();
    app.wait_for_within("label:More", SLOW).unwrap();
    app.wait_until("a terminal on the computer", SLOW, |_| !hc_terminals(&session).is_empty()).unwrap();
    // It starts at the phone's grid, not the core's 200x120.
    let first = hc_terminals(&session)[0]["address"].as_str().unwrap().to_owned();
    let record: serde_json::Value = serde_json::from_slice(&session.hc(&["--json", "get", &first]).stdout).unwrap();
    let record = record.get("terminal").cloned().unwrap_or(record);
    assert!(record["cols"].as_u64().is_some_and(|c| c < 100), "{record:#}");

    // Typing reaches its shell.
    app.wait_for_within("id:terminal-input", SLOW).unwrap();
    app.type_text("id:terminal-input", "touch typed-on-the-phone\n").unwrap();
    if let Err(e) = app.wait_until("the typed command's file", SLOW, |_| session.path("typed-on-the-phone").exists()) {
        let t = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
        let screen = session.hc(&["read", &t]);
        panic!("{e}\nits screen:\n{}{}", String::from_utf8_lossy(&screen.stdout), String::from_utf8_lossy(&screen.stderr));
    }

    // Enter on the bar executes the command without relying on the IME action.
    app.type_text("id:terminal-input", "touch entered-on-the-phone").unwrap();
    assert!(!session.path("entered-on-the-phone").exists());
    app.press("label:Enter").unwrap();
    app.wait_until("the command submitted with Enter", SLOW, |_| session.path("entered-on-the-phone").exists()).unwrap();

    // The key bar's Ctrl holds for the next key, with the keyboard still up.
    app.type_text("id:terminal-input", "sleep 100\n").unwrap();
    // (Headless runs don't play a scroll view's tap dismissing the keyboard:
    // that the bar needs `KeyboardTaps::Always` was found on a phone.)
    app.press("text:Ctrl").unwrap();
    app.type_text("id:terminal-input", "c").unwrap();
    app.wait_until("Ctrl-C to stop the sleep", SLOW, |_| {
        let t = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
        String::from_utf8_lossy(&session.hc(&["read", &t]).stdout).contains("^C")
    })
    .unwrap();

    // A hardware keyboard's Ctrl+C reaches the program too: the shell runs
    // what's typed next at once, not after the sleep.
    app.type_text("id:terminal-input", "sleep 201\n").unwrap();
    app.press_key("ctrl+c").unwrap();
    app.type_text("id:terminal-input", "echo after-$((40+2))\n").unwrap();
    app.wait_until("Ctrl+C from a hardware keyboard", SLOW, |_| {
        let t = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
        String::from_utf8_lossy(&session.hc(&["read", &t]).stdout).lines().any(|l| l.trim() == "after-42")
    })
    .unwrap();

    // A swipe down goes back through history, and let go it goes on.
    app.type_text("id:terminal-input", "seq 1 300\n").unwrap();
    app.wait_until("300 lines", SLOW, |_| {
        let t = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
        String::from_utf8_lossy(&session.hc(&["read", &t]).stdout).lines().any(|l| l.trim() == "300")
    })
    .unwrap();
    // 60 points are 3 lines; flung, it goes several times as far.
    app.drag("id:terminal", point(px(0.0), px(60.0))).unwrap();
    app.wait_until("a fling", SLOW, |s| {
        s.texts().iter().find_map(|t| t.strip_suffix(" lines back")?.parse::<u32>().ok()).is_some_and(|n| n >= 9)
    })
    .unwrap();
    app.press("label:Back to the latest output").unwrap();

    // Held, it pastes what the clipboard has.
    app.set_clipboard(Some("touch pasted-on-the-phone\n".to_owned()));
    app.long_press("id:terminal").unwrap();
    pick(&mut app, "paste");
    app.wait_until("the pasted command's file", SLOW, |_| session.path("pasted-on-the-phone").exists()).unwrap();

    // Or opens the text to select; Copy all copies it.
    app.long_press("id:terminal").unwrap();
    pick(&mut app, "select");
    app.wait_for_within("label:Copy all", SLOW).unwrap();
    app.press("label:Copy all").unwrap();
    let copied = app.screen().clipboard().map(str::to_owned).unwrap_or_default();
    assert!(copied.contains("sleep 100"), "copied: {copied:?}");

    // Held, it offers the links on screen, and opens one.
    app.type_text("id:terminal-input", "echo see https://example.com/pull/7.\n").unwrap();
    app.wait_until("the link on screen", SLOW, |_| {
        let t = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
        String::from_utf8_lossy(&session.hc(&["read", &t]).stdout).lines().any(|l| l.trim() == "see https://example.com/pull/7.")
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    app.long_press("id:terminal").unwrap();
    app.wait_for_within("label:Open example.com/pull/7", SLOW).unwrap();
    pick(&mut app, "link:0");
    app.wait_until("the link opened", SLOW, |_| opened.lock().unwrap().contains(&"https://example.com/pull/7".to_owned())).unwrap();

    // Or a message written whole, sent as one paste and Enter.
    app.long_press("id:terminal").unwrap();
    pick(&mut app, "write");
    app.wait_for_within("id:compose", SLOW).unwrap();
    app.type_text("id:compose", "touch written-on-the-phone").unwrap();
    app.press("label:Send").unwrap();
    app.wait_until("the written command's file", SLOW, |_| session.path("written-on-the-phone").exists()).unwrap();

    // What a program copies (OSC 52) reaches the phone's clipboard.
    app.type_text("id:terminal-input", "printf '\\033]52;c;aGVsbG8=\\a'\n").unwrap();
    app.wait_until("the program's copy", SLOW, |screen| screen.clipboard() == Some("hello")).unwrap();

    // A swipe across goes between tabs: right to the one before, left to the next.
    // (`+` offers a terminal, the agents installed, a space or a worktree.)
    app.press("label:New").unwrap();
    pick(&mut app, "terminal");
    shows_tab(&mut app, 2);
    // The tabs are dots under the name, the shown one selected; a tap shows another.
    app.press("role:tab[1]").unwrap();
    shows_tab(&mut app, 1);
    app.press("role:tab[2]").unwrap();
    shows_tab(&mut app, 2);
    // Put away, a tab is counted after the dots, and comes back from there.
    app.long_press("role:tab[2]").unwrap();
    pick(&mut app, "settle");
    app.wait_for_within("label:1 put away", SLOW).unwrap();
    shows_tab(&mut app, 1);
    app.press("label:1 put away").unwrap();
    app.wait_for_within("label:2", SLOW).unwrap();
    app.press("label:2").unwrap();
    shows_tab(&mut app, 2);
    app.wait_for_within("role:tab[1]", SLOW).unwrap();
    app.drag("id:terminal", point(px(200.0), px(0.0))).unwrap();
    shows_tab(&mut app, 1);
    app.drag("id:terminal", point(px(-200.0), px(0.0))).unwrap();
    shows_tab(&mut app, 2);

    // Files shows the folder the shell is in.
    app.press("label:More").unwrap();
    app.press("label:Files").unwrap();
    app.wait_for_within("text:typed-on-the-phone", SLOW).unwrap();
    // A file opens in its language.
    std::fs::write(session.path("hello.rs"), "fn main() {\n    println!(\"hello from the phone\");\n}\n").unwrap();
    app.wait_for_within("text:hello.rs", SLOW).unwrap();
    app.press("text:hello.rs").unwrap();
    app.wait_until("the file's text", SLOW, |s| s.texts().concat().contains("hello from the phone")).unwrap();
    app.back().unwrap();
    // Markdown is shown as it reads, its source a press away.
    std::fs::write(session.path("notes.md"), "# Read me\n\nSome *words*.\n").unwrap();
    app.wait_for_within("text:notes.md", SLOW).unwrap();
    app.press("text:notes.md").unwrap();
    app.wait_until("the heading", SLOW, |s| s.texts().iter().any(|t| t.trim() == "Read me")).unwrap();
    assert!(!app.screen().texts().concat().contains("# Read me"), "{:?}", app.screen().texts());
    app.press("label:Show the source").unwrap();
    app.wait_until("the source", SLOW, |s| s.texts().concat().contains("# Read me")).unwrap();
    app.back().unwrap();
    app.back().unwrap();

    // Held, the terminal opens a file it names, from the shell's folder.
    app.type_text("id:terminal-input", "echo error at ./hello.rs:2:5\n").unwrap();
    app.wait_until("the path on screen", SLOW, |_| {
        hc_terminals(&session).iter().any(|t| {
            String::from_utf8_lossy(&session.hc(&["read", t["address"].as_str().unwrap_or_default()]).stdout).lines().any(|l| l.trim() == "error at ./hello.rs:2:5")
        })
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    app.long_press("id:terminal").unwrap();
    app.wait_for_within("label:Open hello.rs:2:5", SLOW).unwrap();
    pick(&mut app, "file:0");
    app.wait_until("the named file's text", SLOW, |s| s.texts().concat().contains("hello from the phone")).unwrap();
    app.back().unwrap();

    // Back home: the spaces, and no agents yet; a space opens again.
    app.press("label:Spaces and agents").unwrap();
    app.press("label:Agents").unwrap();
    app.wait_for_within("text:No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not.", SLOW).unwrap();
    // Or any terminal, found by its space's name: a tap shows it.
    app.press("label:Jump to a terminal").unwrap();
    app.type_text("id:search", "hom").unwrap();
    let first = hc_terminals(&session)[0]["address"].as_str().unwrap_or_default().to_owned();
    app.wait_for_within(&format!("id:jump-{first}"), SLOW).unwrap();
    app.type_text("id:search", "e nothing-is-called-this").unwrap();
    app.wait_for_within("text:No terminal is called that", SLOW).unwrap();
    app.back().unwrap();
    app.press("label:Spaces").unwrap();
    app.press("text:home").unwrap();
    app.wait_for_within("label:More", SLOW).unwrap();

    // Wide (a tablet, landscape): split right, then drag the seam left; the
    // panes share the tab anew, and the terminals are sized to them.
    app.set_viewport(size(px(1000.0), px(700.0))).unwrap();
    app.press("label:More").unwrap();
    app.press("label:Split right").unwrap();
    app.wait_for_within("label:Pane size", SLOW).unwrap();
    let new = hc_terminals(&session).last().unwrap()["address"].as_str().unwrap().to_owned();
    let cols = |app: &mut Headless, mark: &str| {
        app.type_text("id:terminal-input >> nth=1", &format!("echo {mark} $(stty size)\n")).unwrap();
        let seen = |_: &_| -> u32 {
            let read = String::from_utf8_lossy(&session.hc(&["read", &new]).stdout).into_owned();
            read.lines().rev().find_map(|l| l.trim().strip_prefix(&format!("{mark} "))?.split(' ').nth(1)?.parse().ok()).unwrap_or(0)
        };
        app.wait_until("the pane's width", SLOW, |s| seen(s) > 0).unwrap();
        seen(app.screen())
    };
    let before = cols(&mut app, "half");
    app.drag("label:Pane size", point(px(-200.0), px(0.0))).unwrap();
    let mut after = 0;
    for round in 0..20 {
        after = cols(&mut app, &format!("wider{round}"));
        if after > before {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(after > before + 10, "{before} columns, then {after}");
    app.set_viewport(size(px(400.0), px(800.0))).unwrap();

    // Settings copies what to tell when something goes wrong, without secrets.
    app.press("label:More").unwrap();
    app.press("label:Settings").unwrap();
    // The fit switch turns (said in the diagnostics), and back.
    app.press("role:switch").unwrap();
    app.press("label:Copy diagnostics").unwrap();
    let said = app.screen().clipboard().map(str::to_owned).unwrap_or_default();
    for part in ["Yonder 0.1.0", "computer: me@127.0.0.1:", "link: {\"state\":\"connected\"", "1 spaces · 2 tabs", "fit false"] {
        assert!(said.contains(part), "{part:?} not in {said}");
    }
    assert!(!said.contains("open sesame") && !said.contains("ssh-ed25519"), "{said}");
    app.press("role:switch").unwrap();
    app.back().unwrap();
    // Its note goes in a few seconds, though the page that said it has.
    app.wait_for_within("text:Copied", SLOW).unwrap();
    app.wait_until("the note gone", SLOW, |s| !s.texts().contains(&"Copied")).unwrap();
    // The key bar is the person's: Esc off, F5 on, and the terminal's bar so.
    app.press("label:More").unwrap();
    app.press("label:Settings").unwrap();
    app.press("text:Key bar").unwrap();
    app.press("label:Take Esc off").unwrap();
    app.press("label:Add F5").unwrap();
    // And one of their own, which types its words and Enter.
    app.type_text("id:own-key", "touch said-on-the-bar").unwrap();
    app.press("label:Add the key").unwrap();
    app.wait_for("label:touch said-on-the-bar, then Enter later").unwrap();
    app.back().unwrap();
    app.back().unwrap();
    app.type_text("id:terminal-input", "").unwrap();
    app.wait_for_within("text:F5", SLOW).unwrap();
    app.wait_for("label:Enter").unwrap();
    assert!(!app.screen().texts().contains(&"Esc"), "{:?}", app.screen().texts());
    app.press("label:touch said-on-the-bar, then Enter").unwrap();
    app.wait_until("the own key's command", SLOW, |_| session.path("said-on-the-bar").exists()).unwrap();

    // What it shows is kept, to draw first next time.
    assert!(storage.entries().iter().any(|(k, _)| k.starts_with("seen.me@127.0.0.1")), "nothing seen kept");

    // Add a machine: the computer's ssh config hosts, and a target it can't reach said so.
    std::fs::create_dir_all(session.path(".ssh")).unwrap();
    std::fs::write(session.path(".ssh/config"), "Host vps\n  HostName 10.0.0.7\n  User can\n").unwrap();
    app.press("label:Spaces and agents").unwrap();
    // What home's menu had is in Settings: the sort of the agents too.
    app.press("label:Settings").unwrap();
    app.wait_for_within("text:Priority", SLOW).unwrap();
    app.press("text:Sort agents").unwrap();
    pick(&mut app, "grouped");
    app.wait_for_within("text:Grouped by space", SLOW).unwrap();
    app.press("text:Sort agents").unwrap();
    pick(&mut app, "priority");
    app.press("label:Add a machine…").unwrap();
    app.wait_for_within("text:can@10.0.0.7", SLOW).unwrap();
    app.type_text("id:machine-target", "nobody@127.0.0.1:1").unwrap();
    app.press("label:Add the machine typed").unwrap();
    app.wait_until("why it wasn't added", SLOW, |screen| screen.texts().iter().any(|t| t.contains("Couldn't add") || t.contains("unreachable") || t.contains("refused"))).unwrap();
    app.back().unwrap();
}

#[test]
fn opens_again_on_the_computer_it_was_on_in_its_language() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = Session::new();
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    let computer = serde_json::json!({
        "name": "127.0.0.1", "host": "127.0.0.1", "port": served.port, "user": "me",
        "fingerprint": served.fingerprint, "password": "open sesame",
    });
    let id = format!("me@127.0.0.1:{}", served.port);
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(MemoryStorage::new(vec![
        ("computers".to_owned(), serde_json::json!([computer]).to_string()),
        ("open".to_owned(), id),
        ("prefs".to_owned(), r#"{"language":"Chinese"}"#.to_owned()),
    ]));
    let mut app = Headless::start(config).unwrap();

    // Straight into the computer, in Chinese.
    app.wait_for_within("label:空间", SLOW).unwrap();
    app.press("label:空间").unwrap();
    app.wait_for_within("text:还没有空间", SLOW).unwrap();
    // Its menu: leaving goes back to the computers, and stays there.
    app.press("label:设置").unwrap();
    app.press("label:断开这台电脑").unwrap();
    app.wait_for_within("text:电脑", SLOW).unwrap();
}

#[test]
fn a_password_refused_is_asked_for_again_not_tried_again() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = Session::new();
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    // The computer's password changed since it was kept.
    let computer = serde_json::json!({
        "name": "127.0.0.1", "host": "127.0.0.1", "port": served.port, "user": "me",
        "fingerprint": served.fingerprint, "password": "the old one",
    });
    let id = format!("me@127.0.0.1:{}", served.port);
    let storage = MemoryStorage::new(vec![("computers".to_owned(), serde_json::json!([computer]).to_string()), ("open".to_owned(), id)]);
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(storage.clone());
    let mut app = Headless::start(config).unwrap();

    app.wait_for_within("text:127.0.0.1 didn't take the password kept for it.", SLOW).unwrap();
    app.press("label:Enter the password").unwrap();
    app.type_text("id:prompt", "open sesame").unwrap();
    app.press("text:Connect").unwrap();
    app.wait_for_within("label:Spaces", SLOW).unwrap();
    app.press("label:Spaces").unwrap();
    app.wait_for_within("text:No spaces yet", SLOW).unwrap();
    // The new one is kept.
    assert!(format!("{:?}", storage.entries()).contains("open sesame"), "{:?}", storage.entries());
}

#[test]
fn what_a_computer_showed_last_is_drawn_while_it_is_not_there() {
    // Nobody answers on its port.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let computer = serde_json::json!({ "name": "127.0.0.1", "host": "127.0.0.1", "port": port, "user": "me", "fingerprint": "SHA256:x" });
    let id = format!("me@127.0.0.1:{port}");
    let root = serde_json::to_value(herdc_model::model::Tab::new("tab", "1", "p1", "t1").root).unwrap();
    let space = |id: &str, name: &str, status: Option<&str>, active: bool| {
        serde_json::json!({
            "id": id, "index": 1, "name": name, "named": true, "folder": "~", "machine": null, "branch": "main",
            "active": active, "status": status,
            "tabs": [{ "id": format!("{id}-tab"), "index": 1, "name": "1", "named": false, "active": true, "status": status,
                "settled": false, "settled_at": null, "changed_at": null, "working_since": null, "focused": "p1", "zoomed": null,
                "root": root, "panes": [] }],
        })
    };
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - 600;
    // proj is on another machine of the computer's.
    let mut proj = space("s1", "proj", Some("blocked"), true);
    proj["machine"] = "gpu".into();
    let seen = serde_json::json!({ "at": at, "view": {
        "link": { "state": "connected", "core": "desk", "version": "0.1.18" }, "home": "/home/me",
        "machines": [{ "name": null, "state": "connected", "layout": "ready", "error": null, "host": "desk" },
            { "name": "gpu", "state": "connected", "layout": "ready", "error": null, "host": "gpu" }],
        "spaces": [proj, space("s2", "notes", Some("idle"), false)],
        "agents": [], "terminals": [], "focused": null,
    }});
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(MemoryStorage::new(vec![
        ("computers".to_owned(), serde_json::json!([computer]).to_string()),
        (format!("seen.{id}"), seen.to_string()),
    ]));
    let mut app = Headless::start(config).unwrap();

    // The computers list its spaces as they were, and when.
    app.wait_for("text:desk").unwrap();
    app.wait_for("label:proj").unwrap();
    app.wait_for("label:notes").unwrap();
    assert!(app.screen().texts().iter().any(|t| t.contains("10m ago")), "{:?}", app.screen().texts());

    app.long_press("label:desk").unwrap();
    pick(&mut app, "rename");
    app.wait_for("id:prompt").unwrap();
    app.clear("id:prompt").unwrap();
    app.type_text("id:prompt", "Studio desk").unwrap();
    app.press("text:OK").unwrap();
    app.wait_for("text:Studio desk").unwrap();
    assert!(!app.screen().texts().iter().any(|t| t.contains("127.0.0.1")));

    // Opened at one, it is drawn before the computer answers (it won't).
    app.press("label:notes").unwrap();
    app.wait_for_within("text:proj", SLOW).unwrap();
    // Said to be on that machine, reached through the computer.
    app.wait_for("text:· gpu · Studio desk").unwrap();
    app.press("label:Spaces and agents").unwrap();
    app.press("label:Spaces").unwrap();
    app.wait_for_within("text:notes", SLOW).unwrap();
    app.long_press("label:Fold or unfold gpu").unwrap();
    app.clear("id:prompt").unwrap();
    app.type_text("id:prompt", "GPU server").unwrap();
    app.press("text:OK").unwrap();
    app.wait_for("label:Fold or unfold GPU server").unwrap();
}

#[test]
fn edits_a_saved_connection_only_after_it_works() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = Session::new();
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    let original = serde_json::json!({
        "name": "Studio", "host": "127.0.0.1", "port": 1, "user": "me",
        "fingerprint": served.fingerprint, "password": "open sesame",
    });
    let storage = MemoryStorage::new(vec![("computers".into(), serde_json::json!([original]).to_string())]);
    let saved = || {
        let raw = storage.entries().into_iter().find(|(k, _)| k == "computers").unwrap().1;
        serde_json::from_str::<Vec<serde_json::Value>>(&raw).unwrap()
    };
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(storage.clone());
    let mut app = Headless::start(config).unwrap();
    app.wait_for("label:Studio").unwrap();
    app.long_press("label:Studio").unwrap();
    pick(&mut app, "edit");
    app.clear("id:port").unwrap();
    app.type_text("id:port", "65536").unwrap();
    app.press("text:Test and save").unwrap();
    app.wait_for("text:Enter a port from 1 to 65535.").unwrap();
    assert_eq!(saved(), vec![original.clone()]);
    app.clear("id:port").unwrap();
    app.type_text("id:port", &served.port.to_string()).unwrap();
    app.type_text("id:password", "incorrect").unwrap();
    app.press("text:Test and save").unwrap();
    app.wait_for_within("text:Wrong user name or password.", SLOW).unwrap();
    assert_eq!(saved(), vec![original]);
    app.clear("id:password").unwrap();
    app.press("text:Test and save").unwrap();
    app.wait_for_within("label:Studio", SLOW).unwrap();
    let entries = saved();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["port"], served.port);
    assert_eq!(entries[0]["password"], "open sesame");
    assert_eq!(entries[0]["fingerprint"], served.fingerprint);

    app.press("label:Studio").unwrap();
    app.wait_for_within("label:Settings", SLOW).unwrap();
    app.press("label:Settings").unwrap();
    app.press("label:Edit connection").unwrap();
    app.clear("id:display name").unwrap();
    app.type_text("id:display name", "Mini").unwrap();
    app.press("text:Test and save").unwrap();
    // Saved once it works, and back (its own field says Mini before that).
    app.wait_until("saved and back", SLOW, |s| s.count("id:display name") == Ok(0)).unwrap();
    app.wait_for_within("text:Mini", SLOW).unwrap();
    assert_eq!(saved()[0]["name"], "Mini");
    app.press("label:Spaces").unwrap();
    // Connected again under its new name, spaces can be made.
    app.wait_until("connected again", SLOW, |s| s.enabled("label:New space") == Ok(true)).unwrap();
    app.press("label:New space").unwrap();
    app.wait_for_within("label:More", SLOW).unwrap();
    app.press("label:More").unwrap();
    app.press("label:Settings").unwrap();
    app.press("label:Edit connection").unwrap();
    app.type_text("id:password", "incorrect").unwrap();
    app.press("text:Test and save").unwrap();
    app.wait_for_within("text:Wrong user name or password.", SLOW).unwrap();
    // Nothing saved: back is the way out (no Cancel under a back arrow).
    app.back().unwrap();
    app.back().unwrap();
    app.wait_for_within("id:terminal-input", SLOW).unwrap();
    app.type_text("id:terminal-input", "touch still-connected\n").unwrap();
    app.wait_until("original terminal still accepts input", SLOW, |_| session.path("still-connected").exists()).unwrap();

}

#[test]
fn another_session_is_a_computer_of_its_own() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = Session::new();
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    let storage = MemoryStorage::new(Vec::<(String, String)>::new());
    config.storage = Some(storage.clone());
    let mut app = Headless::start(config).unwrap();
    app.wait_for("text:Connect").unwrap();
    app.type_text("id:display name", "Studio · two").unwrap();
    app.type_text("id:computer", "127.0.0.1").unwrap();
    app.type_text("id:user", "me").unwrap();
    app.clear("id:port").unwrap();
    app.type_text("id:password", "nope").unwrap();

    // Trying, Connect says what it does and takes no press; something
    // that never answers is given up on.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    app.type_text("id:port", &silent.local_addr().unwrap().port().to_string()).unwrap();
    app.press("text:Connect").unwrap();
    app.wait_for("text:Reaching the computer…").unwrap();
    assert!(!app.screen().enabled("text:Reaching the computer…").unwrap());
    app.wait_for_within("text:127.0.0.1 doesn't answer.", SLOW).unwrap();
    assert!(app.screen().enabled("text:Connect").unwrap());
    drop(silent);
    app.clear("id:port").unwrap();
    app.type_text("id:port", &served.port.to_string()).unwrap();
    // Hidden, unless asked.
    app.press("label:Show the password").unwrap();
    app.wait_for("text:Hide").unwrap();

    // Words a shell would take apart are not a session.
    // A session is a seldom thing: under More options.
    app.press("label:More options").unwrap();
    app.type_text("id:session", "two; ls").unwrap();
    app.press("text:Connect").unwrap();
    app.wait_for("text:A session is letters, digits and . _ - / only.").unwrap();

    app.clear("id:session").unwrap();
    app.type_text("id:session", "two").unwrap();
    let fingerprint = Box::leak(served.fingerprint.clone().into_boxed_str());
    answer(&mut app, "Trust", fingerprint);

    // A wrong password is said so, at once.
    app.press("text:Connect").unwrap();
    app.wait_for_within("text:Wrong user name or password.", SLOW).unwrap();

    app.clear("id:password").unwrap();
    app.type_text("id:password", "open sesame").unwrap();
    app.press("text:Connect").unwrap();
    app.wait_for_within("label:Spaces", SLOW).unwrap();
    app.press("label:Spaces").unwrap();
    app.wait_for_within("text:No spaces yet", SLOW).unwrap();
    app.press("role:button[New space]").unwrap();
    app.wait_for_within("label:More", SLOW).unwrap();
    // Its terminal is that session's: the default one has none.
    std::thread::sleep(Duration::from_secs(1));
    assert!(hc_terminals(&session).is_empty(), "{:?}", hc_terminals(&session));
    let stored = format!("{:?}", storage.entries());
    assert!(stored.contains(&format!("me@127.0.0.1:{}/two", served.port)), "{stored}");

    // At home, a space's handle drags it below the next.
    app.press("label:New").unwrap();
    pick(&mut app, "terminal");
    shows_tab(&mut app, 2);
    app.press("label:Spaces and agents").unwrap();
    app.long_press("text:home").unwrap();
    pick(&mut app, "rename");
    app.wait_for_within("text:Rename space", SLOW).unwrap();
    app.type_text("id:prompt", "first space").unwrap();
    app.press("text:OK").unwrap();
    app.press("label:New space").unwrap();
    app.wait_until("a second space", SLOW, |_| hc_terminals_of(&session, "two") == 3).unwrap();
    app.press("label:Spaces and agents").unwrap();
    let order = |screen: &nsh_testing::Screen| {
        let texts = screen.texts();
        (texts.iter().rposition(|t| *t == "first space"), texts.iter().rposition(|t| *t == "home"))
    };
    app.wait_until("both spaces listed", SLOW, |s| matches!(order(s), (Some(a), Some(b)) if a < b)).unwrap();
    // Handles only show while reordering, from a space's menu.
    assert_eq!(app.screen().count("label:Move first space"), Ok(0));
    app.long_press("text:first space").unwrap();
    pick(&mut app, "reorder");
    app.wait_for("label:Done").unwrap();
    app.drag("label:Move first space", point(px(0.0), px(80.0))).unwrap();
    app.wait_until("the first space below the second", SLOW, |s| matches!(order(s), (Some(a), Some(b)) if a > b)).unwrap();
    app.press("label:Done").unwrap();
    app.wait_for("label:New space").unwrap();

    // Held, a space renames: home and the computer have the new name.
    app.long_press("text:home").unwrap();
    pick(&mut app, "rename");
    app.wait_for_within("text:Rename space", SLOW).unwrap();
    app.type_text("id:prompt", "named").unwrap();
    app.press("text:OK").unwrap();
    app.wait_for_within("text:named", SLOW).unwrap();

    // Opened again, it is on the space it showed last (not the first).
    app.press("text:first space").unwrap();
    let on_first = || {
        storage.entries().iter().filter(|(k, _)| k.starts_with("seen.")).any(|(_, v)| {
            let seen: serde_json::Value = serde_json::from_str(v).unwrap_or_default();
            seen["view"]["spaces"].as_array().into_iter().flatten().any(|s| s["name"] == "first space" && s["active"] == true)
        })
    };
    // Storage changes nothing on screen: looked at every second.
    let since = std::time::Instant::now();
    while !on_first() {
        assert!(since.elapsed() < Duration::from_secs(30), "first space not kept as the space seen last");
        std::thread::sleep(Duration::from_secs(1));
    }
    // With the terminals the phone started there.
    assert!(storage.entries().iter().any(|(k, v)| k.starts_with("seen.") && v.contains(r#""started":["t1","t2","t3"]"#)), "{:?}", storage.entries());
    drop(app);
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(storage.clone());
    let mut app = Headless::start(config).unwrap();
    // What it showed last is drawn first: looked at once the computer has answered.
    std::thread::sleep(Duration::from_secs(5));
    // Its workspace: the space's name over the computer (its tabs on the strip).
    app.wait_until("open on first space", SLOW, |s| s.texts().windows(2).any(|w| w[0] == "first space" && w[1].ends_with("Studio · two"))).unwrap();
}

/// How many terminals session `name` of the test computer has.
fn hc_terminals_of(session: &Session, name: &str) -> usize {
    let out = session.command(herdc_testkit::core_binary()).env("HC_SESSION", name).args(["--json", "ls"]).output().unwrap();
    serde_json::from_slice::<serde_json::Value>(&out.stdout).map_or(0, |v| v["terminals"].as_array().map_or(0, Vec::len))
}

#[test]
fn a_terminal_the_desktop_holds_is_the_phones_once_shown() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = std::sync::Arc::new(Session::new());
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    // The desktop holds a space's terminal at its wide grid.
    let _entered = runtime.enter();
    let connect = {
        let session = session.clone();
        std::sync::Arc::new(move || {
            let session = session.clone();
            Box::pin(async move { Ok(session.connect().await) }) as _
        })
    };
    let desktop = herdc_engine::Engine::start(connect, herdc_engine::Options { size: Some((145, 45)), ..Default::default() });
    let made = runtime.block_on(async {
        let mut tries = 0;
        loop {
            match desktop.run("space.new", serde_json::json!({"name": "wide"})).await {
                Ok(made) => break made,
                Err(_) if tries < 100 => (tries += 1, tokio::time::sleep(Duration::from_millis(100)).await).1,
                Err(e) => panic!("{e}"),
            }
        }
    });
    let terminal = made["terminal"].as_str().unwrap().to_owned();
    // Another, the one the desktop shows now.
    let other = runtime.block_on(desktop.run("space.new", serde_json::json!({"name": "other"}))).unwrap()["terminal"].as_str().unwrap().to_owned();
    // As the phone saw it last, at the wide one: drawn at once, before the
    // computer answers.
    let mut view = desktop.watch();
    let view = runtime.block_on(async {
        while view.borrow_and_update().spaces.iter().filter(|s| s.tabs.iter().any(|t| t.panes.iter().any(|p| p.live))).count() < 2 {
            view.changed().await.unwrap();
        }
        view.borrow().clone()
    });
    let mut seen = serde_json::json!({ "at": 1, "view": *view });
    for space in seen["view"]["spaces"].as_array_mut().unwrap() {
        space["active"] = (space["name"] == "wide").into();
    }
    let hold = |t: &str| desktop.viewer(t, herdc_engine::Mode::Type { cols: 145, rows: 45 }, herdc_term::Palette::default());
    let _held = (hold(&terminal), hold(&other));
    let cols_of = |t: &str| {
        let r: serde_json::Value = serde_json::from_slice(&session.hc(&["--json", "get", t]).stdout).unwrap_or_default();
        r.get("terminal").unwrap_or(&r)["cols"].as_u64().unwrap_or(0)
    };
    let cols = || cols_of(&terminal);
    let mut app = Headless::start({
        let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
        let computer = serde_json::json!({
            "name": "127.0.0.1", "host": "127.0.0.1", "port": served.port, "user": "me",
            "fingerprint": served.fingerprint, "password": "open sesame",
        });
        config.storage = Some(MemoryStorage::new(vec![
            ("computers".to_owned(), serde_json::json!([computer]).to_string()),
            ("open".to_owned(), format!("me@127.0.0.1:{}", served.port)),
            (format!("seen.me@127.0.0.1:{}", served.port), seen.to_string()),
        ]));
        config
    })
    .unwrap();
    app.wait_until("held at the desktop's grid", SLOW, |_| cols() == 145 && cols_of(&other) == 145).unwrap();

    // Opened straight at it, the phone takes it at its own grid: the
    // desktop's, shrunk to fit, is too small to read.
    app.wait_for_within("text:wide", SLOW).unwrap();
    app.wait_until("the terminal at the phone's grid", SLOW, |_| (1..100).contains(&cols())).unwrap();
    // The desktop's space, not drawn a moment before the phone's is selected
    // again, keeps its terminal as it is.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(cols_of(&other), 145);
}

#[test]
fn home_makes_no_space_before_the_computer_has_said_its_layout() {
    // Nobody answers on its port, and nothing of it is remembered.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let computer = serde_json::json!({ "name": "desk", "host": "127.0.0.1", "port": port, "user": "me", "fingerprint": "SHA256:x" });
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(MemoryStorage::new(vec![
        ("computers".to_owned(), serde_json::json!([computer]).to_string()),
        ("open".to_owned(), format!("me@127.0.0.1:{port}")),
    ]));
    let mut app = Headless::start(config).unwrap();
    // Home says it's loading, not that there are no spaces, and its New
    // space takes no press: one made now would go nowhere.
    app.wait_for("label:Settings").unwrap();
    app.press("label:Spaces").unwrap();
    app.wait_for("text:Loading…").unwrap();
    let texts = app.screen().texts();
    assert!(!texts.contains(&"No spaces yet"), "{texts:?}");
    assert!(!app.screen().enabled("label:New space").unwrap());
}

#[test]
fn an_agent_s_row_pulled_left_shows_its_buttons_and_stopping_is_asked() {
    // Drawn from what the computer showed last; nobody answers on its port.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let computer = serde_json::json!({ "name": "desk", "host": "127.0.0.1", "port": port, "user": "me", "fingerprint": "SHA256:x" });
    let root = serde_json::to_value(herdc_model::model::Tab::new("tab", "1", "p1", "t1").root).unwrap();
    let agent = serde_json::json!({ "address": "t1", "pane": "p1", "space": "s1", "space_name": "proj", "machine": null,
        "tab": "tab", "tab_index": 1, "tab_name": "1", "status": "working", "app": null, "program": {},
        "title": "Fix the build", "seq": 1, "pinned": false, "focused": false });
    let seen = serde_json::json!({ "at": 0, "view": {
        "link": { "state": "connected", "core": "desk", "version": "0.1.18" }, "home": "/home/me",
        "machines": [{ "name": null, "state": "connected", "layout": "ready", "error": null, "host": "desk" }],
        "spaces": [{ "id": "s1", "index": 1, "name": "proj", "named": true, "folder": "~", "machine": null, "branch": "main",
            "active": true, "status": "working",
            "tabs": [{ "id": "tab", "index": 1, "name": "1", "named": false, "active": true, "status": "working",
                "settled": false, "settled_at": null, "changed_at": null, "working_since": null, "focused": "p1", "zoomed": null,
                "root": root, "panes": [] }] }],
        "agents": [agent], "terminals": [], "focused": null,
    }});
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(MemoryStorage::new(vec![
        ("computers".to_owned(), serde_json::json!([computer]).to_string()),
        (format!("seen.me@127.0.0.1:{port}"), seen.to_string()),
    ]));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("label:proj").unwrap();
    app.press("label:proj").unwrap();
    app.wait_for_within("label:Spaces and agents", SLOW).unwrap();
    app.press("label:Spaces and agents").unwrap();
    app.wait_for_within("text:Fix the build", SLOW).unwrap();

    // A little to the left: back, closed.
    app.drag("text:Fix the build", point(px(-30.0), px(0.0))).unwrap();
    app.wait(Duration::from_millis(500)).unwrap();
    assert_eq!(app.screen().count("id:swipe-stop"), Ok(0));
    // Far enough: open on its buttons, and it stays so; nothing asked yet.
    app.drag("text:Fix the build", point(px(-160.0), px(0.0))).unwrap();
    app.wait(Duration::from_millis(500)).unwrap();
    app.wait_for("id:swipe-stop").unwrap();
    app.wait_for("id:swipe-pin").unwrap();
    assert!(app.screen().dialogs().is_empty());
    // Its Stop asks first (and the row is left as it was, closed).
    answer(&mut app, "Cancel", "Stop this agent?");
    app.press("id:swipe-stop").unwrap();
    app.wait_until("asked to stop it", SLOW, |s| s.dialogs().len() == 1).unwrap();
    app.wait_until("closed again", SLOW, |s| s.count("id:swipe-stop") == Ok(0)).unwrap();
    app.wait_for("text:Fix the build").unwrap();
    // Opened again, a tap on the row closes it (and shows nothing).
    app.drag("text:Fix the build", point(px(-160.0), px(0.0))).unwrap();
    app.wait_for("id:swipe-stop").unwrap();
    app.press("text:Fix the build").unwrap();
    app.wait_until("closed by a tap", SLOW, |s| s.count("id:swipe-stop") == Ok(0)).unwrap();
    app.wait_for("text:Fix the build").unwrap();
}

#[test]
fn an_agent_made_the_main_one_has_a_tab_of_its_own_and_the_app_opens_on_it() {
    // Drawn from what the computer showed last; nobody answers on its port.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let computer = serde_json::json!({ "name": "desk", "host": "127.0.0.1", "port": port, "user": "me", "fingerprint": "SHA256:x" });
    let root = serde_json::to_value(herdc_model::model::Tab::new("tab", "1", "p1", "t1").root).unwrap();
    let agent = serde_json::json!({ "address": "t1", "pane": "p1", "space": "s1", "space_name": "proj", "machine": null,
        "tab": "tab", "tab_index": 1, "tab_name": "1", "status": "working", "app": "pi", "program": {},
        "title": "Fix the build", "seq": 1, "pinned": false, "focused": false });
    // What it handed work to: a terminal it started.
    let helper = serde_json::json!({ "address": "t2", "parent": "t1", "state": "live", "status": "working", "name": "helper", "program": {} });
    let seen = serde_json::json!({ "at": 0, "view": {
        "link": { "state": "connected", "core": "desk", "version": "0.1.18" }, "home": "/home/me",
        "machines": [{ "name": null, "state": "connected", "layout": "ready", "error": null, "host": "desk" }],
        "spaces": [{ "id": "s1", "index": 1, "name": "proj", "named": true, "folder": "~", "machine": null, "branch": "main",
            "active": true, "status": "working",
            "tabs": [{ "id": "tab", "index": 1, "name": "1", "named": false, "active": true, "status": "working",
                "settled": false, "settled_at": null, "changed_at": null, "working_since": null, "focused": "p1", "zoomed": null,
                "root": root, "panes": [] }] }],
        "agents": [agent], "terminals": [helper], "focused": null,
    }});
    let storage = MemoryStorage::new(vec![
        ("computers".to_owned(), serde_json::json!([computer]).to_string()),
        (format!("seen.me@127.0.0.1:{port}"), seen.to_string()),
    ]);
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(storage.clone());
    let mut app = Headless::start(config).unwrap();
    app.wait_for("label:proj").unwrap();
    app.press("label:proj").unwrap();
    app.wait_for_within("label:Spaces and agents", SLOW).unwrap();
    app.press("label:Spaces and agents").unwrap();

    // None yet: its tab says what it is for.
    app.wait_for_within("label:Main", SLOW).unwrap();
    app.press("label:Main").unwrap();
    app.wait_for_within("id:choose-lead", SLOW).unwrap();
    // An agent's menu makes it the one, and its tab shows: the agent, what
    // works for it, a field to write to it.
    app.press("label:Agents").unwrap();
    app.wait_for_within("text:Fix the build", SLOW).unwrap();
    app.long_press("text:Fix the build").unwrap();
    pick(&mut app, "lead");
    app.wait_for_within("label:Main agent: proj", SLOW).unwrap();
    app.wait_for("id:talk").unwrap();
    app.wait_for("label:helper, sub-agent").unwrap();
    // Pi's, it shows as a conversation; its terminal is a press away.
    app.press("label:Show the terminal").unwrap();
    app.wait_for_within("id:terminal", SLOW).unwrap();
    app.press("label:Show the conversation").unwrap();
    app.wait_until("the conversation again", SLOW, |s| s.count("id:terminal") == Ok(0)).unwrap();
    // Not sent (nobody answers here): said so, and the words kept.
    app.type_text("id:talk", "hello").unwrap();
    app.press("label:Send").unwrap();
    app.wait_for_within("text:Not sent", SLOW).unwrap();
    drop(app);

    // Opened again, the app shows it, not the space it was on.
    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(storage);
    let mut app = Headless::start(config).unwrap();
    app.wait_for_within("id:talk", SLOW).unwrap();
    assert_eq!(app.screen().count("label:Spaces and agents"), Ok(0));
    // Settings let it go.
    app.press("label:Settings").unwrap();
    app.wait_for_within("text:Main agent", SLOW).unwrap();
    app.press("text:Main agent").unwrap();
    pick(&mut app, "none");
    app.wait_for_within("text:None", SLOW).unwrap();
}

/// A Pi as far as a phone can tell: it says it is Pi and how it is, and
/// keeps its session in a file as Pi does. A line said to it is a turn
/// (a step, then words); `ask` asks in its terminal until answered; `slow`
/// works until a key comes (Esc, stopping it).
const FAKE_PI: &str = r#"f="$HOME/.pi/agent/sessions/--fake--/2026-11-05T17-59-33-091Z_fake.jsonl"
mkdir -p "${f%/*}"
printf '{"type":"session","id":"s"}\n' > "$f"
n=0; p=""
say() { printf '\033]7501;state=%s:app=pi\033\\' "$1"; }
add() { n=$((n+1)); printf '{"type":"message","id":"e%s","parentId":"%s","timestamp":"2026-11-05T17:59:34.063Z","message":%s}\n' "$n" "$p" "$1" >> "$f"; p="e$n"; }
you() { add "{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"$1\"}]}"; }
reply() { add "{\"role\":\"assistant\",\"stopReason\":\"${2:-stop}\",\"content\":[{\"type\":\"text\",\"text\":\"$1\"}]}"; }
say idle
while IFS= read -r m; do
  case "$m" in
    ask) you ask; reply "Which one?"; say blocked; printf 'Pick one:\n> yes\n  no\n'; IFS= read -r a; reply Answered; say idle;;
    slow) you slow; say working; stty -icanon min 1 time 0; dd bs=1 count=1 >/dev/null 2>&1; stty icanon; reply "" aborted; say idle;;
    *) you "$m"; say working
       add '{"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"echo hi"}}]}'
       add '{"role":"toolResult","toolCallId":"c1","content":[{"type":"text","text":"hi"}]}'
       reply "Done with $m"; say idle;;
  esac
done"#;

#[test]
fn a_pi_main_agent_is_talked_to_as_a_conversation() {
    use herdc_engine::{Engine, Options as Screen};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let session = std::sync::Arc::new(Session::new());
    let served = runtime.block_on(computer::serve(&session, Options::default())).unwrap();
    // The agent, in a space of its own, started from another screen.
    let address = runtime.block_on(async {
        let made = session.clone();
        let screen = Engine::start(std::sync::Arc::new(move || {
            let session = made.clone();
            Box::pin(async move { Ok(session.connect().await) })
        }), Screen { size: Some((80, 30)), ..Default::default() });
        let mut watch = screen.watch();
        tokio::time::timeout(SLOW, async {
            while !watch.borrow_and_update().machines.first().is_some_and(|m| m.layout == "ready") {
                watch.changed().await.unwrap();
            }
        }).await.unwrap();
        let made = screen.run("space.new", serde_json::json!({"argv": ["sh", "-c", FAKE_PI]})).await.unwrap();
        let address = made["terminal"].as_str().unwrap().to_owned();
        screen.client().unwrap().call("terminal.label", serde_json::json!({"terminal": address, "labels": {"alma.session": "pi:id:fake"}})).await.unwrap();
        screen.close();
        address
    });

    let mut config = EngineConfig::new(Root::of::<herdc_app::App>());
    config.storage = Some(MemoryStorage::new(Vec::<(String, String)>::new()));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("text:Connect").unwrap();
    app.type_text("id:display name", "Studio").unwrap();
    app.type_text("id:computer", "127.0.0.1").unwrap();
    app.type_text("id:user", "me").unwrap();
    app.clear("id:port").unwrap();
    app.type_text("id:port", &served.port.to_string()).unwrap();
    app.type_text("id:password", "open sesame").unwrap();
    let fingerprint = Box::leak(served.fingerprint.clone().into_boxed_str());
    answer(&mut app, "Trust", fingerprint);
    app.press("text:Connect").unwrap();
    app.wait_for_within("label:Settings", SLOW).unwrap();
    app.press("label:Settings").unwrap();
    app.wait_for_within("text:Main agent", SLOW).unwrap();
    app.press("text:Main agent").unwrap();
    pick(&mut app, &address);

    // Its session, read: empty yet. Said to, the words wait, then show
    // as it wrote them down, with the step it took and the time.
    app.wait_for_within("id:talk", SLOW).unwrap();
    app.wait_for_within("label:Send a picture", SLOW).unwrap();
    app.type_text("id:talk", "hello").unwrap();
    app.press("label:Send").unwrap();
    app.wait_for_within("text:Sending…", SLOW).unwrap();
    app.wait_for_within("text:Done with hello", SLOW).unwrap();
    app.wait_until("no longer waiting", SLOW, |s| s.count("text:Sending…") == Ok(0) && s.count("text:Sent") == Ok(0)).unwrap();
    app.wait_for("text:11/5 17:59").unwrap();
    // Its step, whole, on a page of its own.
    app.press("label:bash echo hi").unwrap();
    app.wait_for_within("id:step-input", SLOW).unwrap();
    app.wait_for("text:echo hi").unwrap();
    app.wait_for("text:hi").unwrap();
    app.back().unwrap();

    // Asking in its terminal: the question shows here, and what is
    // written answers it there.
    app.wait_for_within("id:talk", SLOW).unwrap();
    app.type_text("id:talk", "ask").unwrap();
    app.press("label:Send").unwrap();
    app.wait_for_within("id:asked", SLOW).unwrap();
    app.wait_for("id:question").unwrap();
    app.press("label:Down").unwrap();
    app.type_text("id:talk", "yes").unwrap();
    app.press("label:Send").unwrap();
    app.wait_for_within("text:Answered", SLOW).unwrap();
    app.wait_until("not asking", SLOW, |s| s.count("id:asked") == Ok(0)).unwrap();

    // Working, it is stopped from here.
    app.type_text("id:talk", "slow").unwrap();
    app.press("label:Send").unwrap();
    app.wait_for_within("id:stop", SLOW).unwrap();
    app.press("id:stop").unwrap();
    app.wait_for_within("text:Stopped", SLOW).unwrap();

    // A picture goes beside it, its path in what is written.
    app.on_pick(|_| vec![nsh_core::picker::PickedFile::new("shot.png", "image/png", nsh_core::ImageData::new(b"\x89PNG".to_vec()))]);
    app.press("id:attach").unwrap();
    app.wait_until("its path written", SLOW, |s| s.input_text("id:talk").ok().flatten().is_some_and(|t| t.ends_with("shot.png"))).unwrap();
    let path = app.screen().input_text("id:talk").unwrap().unwrap().to_owned();
    assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG");
}
