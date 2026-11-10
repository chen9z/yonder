//! Two screens on one isolated herdr-core: what one does, the other shows.

use herdc_engine::{Connector, Engine, Mode, Options, View, Viewer};
use herdc_term::Palette;
use herdc_testkit::Session;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn connector(session: &Arc<Session>) -> Connector {
    let session = session.clone();
    Arc::new(move || {
        let session = session.clone();
        Box::pin(async move { Ok(session.connect().await) })
    })
}

fn screen(session: &Arc<Session>) -> Engine {
    Engine::start(connector(session), Options { size: Some((100, 30)), ..Default::default() })
}

/// The view once `ok` holds, failing after 20 s.
async fn until(engine: &Engine, what: &str, ok: impl Fn(&View) -> bool) -> Arc<View> {
    let mut watch = engine.watch();
    let found = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let view = watch.borrow_and_update().clone();
            if ok(&view) {
                return view;
            }
            if watch.changed().await.is_err() {
                panic!("engine stopped");
            }
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out waiting for {what}: {:#?}", engine.view()))
}

async fn ready(engine: &Engine) {
    until(engine, "the layout", |v| v.connected() && v.machines.first().is_some_and(|m| m.layout == "ready")).await;
}

/// Waits until the viewer shows what `ok` wants, failing after 10 s.
async fn shows(viewer: &Viewer, what: &str, ok: impl Fn(&Viewer) -> bool) {
    let mut changes = viewer.changes();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ok(viewer) {
        assert!(tokio::time::timeout_at(deadline, changes.changed()).await.is_ok(), "timed out waiting for {what}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn spaces_tabs_and_panes_are_shared_with_every_screen() {
    let session = Arc::new(Session::new());
    let (phone, desktop) = (screen(&session), screen(&session));
    ready(&phone).await;
    ready(&desktop).await;

    let made = phone.run("space.new", json!({"name": "web"})).await.unwrap();
    let terminal = made["terminal"].as_str().unwrap().to_owned();
    let v = until(&desktop, "the space", |v| v.spaces.iter().any(|s| s.name == "web" && s.tabs[0].panes[0].live)).await;
    assert_eq!(v.spaces[0].tabs[0].panes[0].terminal.as_deref(), Some(terminal.as_str()));

    let split = phone.run("pane.split", json!({"direction": "right"})).await.unwrap();
    assert!(split["terminal"].is_string());
    until(&desktop, "the split", |v| v.spaces.iter().any(|s| s.tabs[0].panes.len() == 2)).await;
    // Focus is each screen's own: going left on the phone moves only its focus.
    let right = split["pane"].as_str().unwrap().to_owned();
    phone.run("pane.go", json!({"direction": "left"})).await.unwrap();
    let v = phone.view();
    assert_ne!(v.focused.as_ref().and_then(|f| f.pane.clone()), Some(right.clone()));

    desktop.run("space.select", json!({"index": 1})).await.unwrap();
    desktop.run("tab.rename", json!({"name": "api"})).await.unwrap();
    until(&phone, "the rename", |v| v.active_tab().is_some_and(|t| t.name == "api")).await;

    // What the desktop asks in a dialog comes back as a question.
    let asked = phone.run("tab.rename", json!({})).await.unwrap();
    assert_eq!(asked["ask"]["kind"], "prompt");
    assert_eq!(asked["ask"]["value"], "api");
    let asked = phone.run("space.close", json!({})).await.unwrap();
    assert_eq!(asked["ask"]["kind"], "confirm");
    assert_eq!(asked["ask"]["detail"], "web \u{2014} 1 tab, 2 terminals");
    let command = &asked["ask"]["command"];
    phone.run(command["name"].as_str().unwrap(), command["args"].clone()).await.unwrap();
    until(&desktop, "the close", |v| v.spaces.is_empty()).await;
    let client = phone.client().unwrap();
    for _ in 0..100 {
        let record = client.call("terminal.get", json!({"terminal": terminal})).await;
        if record.is_err() || record.unwrap()["state"] != "live" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("closing the space did not end its terminals");
}

#[tokio::test(flavor = "multi_thread")]
async fn terminals_labeled_elsewhere_are_adopted() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let made = phone.run("space.new", json!({})).await.unwrap();
    let space = made["space"].as_str().unwrap();

    // An agent (or a script) starts one in that space, in a tab of its own.
    let client = phone.client().unwrap();
    let labels = json!({"herdr.space": space, "herdr.tab": "tbagent"});
    let run = client.call("terminal.run", json!({"argv": ["cat"], "labels": labels})).await.unwrap();
    let address = run["address"].as_str().unwrap().to_owned();
    let v = until(&phone, "the adoption", |v| v.spaces[0].tabs.iter().any(|t| t.id == "tbagent")).await;
    let tab = v.tab("tbagent").unwrap();
    assert_eq!(tab.panes[0].terminal.as_deref(), Some(address.as_str()));
    // Shown where it was, without taking the screen.
    assert!(!tab.active);
    let record = client.call("terminal.get", json!({"terminal": address})).await.unwrap();
    assert_eq!(record["keep"], true);
    // Where it is follows, written after the adoption.
    eventually("its place written", async || {
        let record = client.call("terminal.get", json!({"terminal": address})).await.unwrap();
        record["labels"]["where"].as_str().is_some_and(|w| w.ends_with("/ 2"))
    })
    .await;
}

const AGENT: &str = r#"while read s; do printf '\033]7501;state=%s:app=codex\033\\' "$s"; done"#;

#[tokio::test(flavor = "multi_thread")]
async fn agents_ring_when_they_finish_unwatched_and_are_seen_when_shown() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let mut notes = phone.notes();
    let made = phone.run("space.new", json!({"argv": ["sh", "-c", AGENT]})).await.unwrap();
    let agent = made["terminal"].as_str().unwrap().to_owned();
    let viewer = phone.viewer(&agent, Mode::Watch, Palette::default());

    viewer.input(b"working\r");
    let v = until(&phone, "working", |v| v.agents.iter().any(|a| a.address == agent && a.status == "working")).await;
    assert_eq!(v.agents[0].program["key"], "codex");

    // Another tab in front: the agent finishing is news.
    phone.run("tab.new", json!({})).await.unwrap();
    viewer.input(b"idle\r");
    until(&phone, "done", |v| v.agents.iter().any(|a| a.address == agent && a.status == "done")).await;
    let note = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let n = notes.recv().await.unwrap();
            if n.alert.is_some() {
                return n;
            }
        }
    })
    .await
    .expect("an alert");
    assert_eq!((note.alert, note.toast), (Some("done"), true));
    assert_eq!(note.action.as_ref().unwrap().args["terminal"], agent.as_str());

    // Shown again: seen here, and shared with every screen.
    phone.run(&note.action.as_ref().unwrap().name, note.action.as_ref().unwrap().args.clone()).await.unwrap();
    until(&phone, "seen", |v| v.agents.iter().any(|a| a.address == agent && a.status == "idle")).await;
    // A screen that came later has when it changed on the computer, not
    // when it first saw it.
    let record = session.json(&["get", &agent]);
    let changed = record.get("terminal").unwrap_or(&record)["status_ms"].as_u64();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let late = screen(&session);
    let seen = until(&late, "listed, later", |v| v.agents.iter().any(|a| a.address == agent)).await;
    assert_eq!(Some(seen.agents.iter().find(|a| a.address == agent).unwrap().since), changed);
    drop(late);
    let client = phone.client().unwrap();
    for _ in 0..100 {
        let r = client.call("terminal.get", json!({"terminal": agent})).await.unwrap();
        if r["labels"]["herdr.seen"].as_str().is_some_and(|s| s != "0") && r["labels"]["herdr.seen"] == json!(r["turn"].to_string()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("herdr.seen was not shared");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watching_viewer_types_at_the_size_it_found() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let client = phone.client().unwrap();
    let run = client.call("terminal.run", json!({"argv": ["cat"]})).await.unwrap();
    let address = run["address"].as_str().unwrap().to_owned();
    let record = client.call("terminal.get", json!({"terminal": address})).await.unwrap();
    let grid = (record["cols"].as_u64().unwrap() as u16, record["rows"].as_u64().unwrap() as u16);
    let viewer = phone.viewer(&address, Mode::Watch, Palette::default());
    shows(&viewer, "the attach", |v| v.info().attached).await;
    assert_eq!(viewer.with_screen(|s| (s.cols(), s.rows())), grid);
    // Key by key, as a phone's keyboard types: none lost while the
    // watching stream gives way to a typing one.
    for key in b"hello from the phone\r" {
        viewer.input(&[*key]);
    }
    shows(&viewer, "the echo", |v| v.snapshot().text().contains("hello from the phone")).await;
    assert_eq!(viewer.mode(), Mode::Type { cols: grid.0, rows: grid.1 });
    let record = client.call("terminal.get", json!({"terminal": address})).await.unwrap();
    assert_eq!((record["cols"].as_u64(), record["rows"].as_u64()), (Some(grid.0 as u64), Some(grid.1 as u64)));

    // Fitting it to the phone resizes the terminal.
    viewer.set_mode(Mode::Type { cols: 60, rows: 20 });
    for _ in 0..100 {
        let r = client.call("terminal.get", json!({"terminal": address})).await.unwrap();
        if r["cols"] == 60 && r["rows"] == 20 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the terminal was not resized");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_core_is_found_again_with_its_terminals() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let made = phone.run("space.new", json!({"name": "kept"})).await.unwrap();
    let terminal = made["terminal"].as_str().unwrap().to_owned();
    let _ = phone.client().unwrap().call("server.restart", json!({})).await;
    until(&phone, "the loss", |v| !v.connected()).await;
    ready(&phone).await;
    let v = until(&phone, "the space back", |v| v.spaces.iter().any(|s| s.name == "kept" && s.tabs[0].panes[0].live)).await;
    assert_eq!(v.spaces[0].tabs[0].panes[0].terminal.as_deref(), Some(terminal.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn new_terminals_start_at_the_screens_grid_and_keep_it() {
    let session = Arc::new(Session::new());
    let phone = Engine::start(connector(&session), Options { size: Some((57, 20)), display: json!({"cell": [7, 16]}), ..Default::default() });
    ready(&phone).await;
    let grid = |terminal: String| {
        let phone = phone.clone();
        async move {
            let r = phone.client().unwrap().call("terminal.get", json!({"terminal": terminal})).await.unwrap();
            (r["cols"].as_u64().unwrap(), r["rows"].as_u64().unwrap())
        }
    };
    let made = phone.run("space.new", json!({})).await.unwrap();
    let first = made["terminal"].as_str().unwrap().to_owned();
    assert_eq!(grid(first.clone()).await, (57, 20));
    phone.set_size(40, 12);
    let made = phone.run("tab.new", json!({})).await.unwrap();
    let second = made["terminal"].as_str().unwrap().to_owned();
    assert_eq!(grid(second.clone()).await, (40, 12));

    // Shown, it stays the phone's: a watcher alone would leave it headless.
    assert_eq!(phone.first_mode(&second, false), Mode::Type { cols: 40, rows: 12 });
    let viewer = phone.viewer(&second, phone.first_mode(&second, false), Palette::default());
    let mut changes = viewer.changes();
    while !viewer.info().attached {
        changes.changed().await.unwrap();
    }
    assert_eq!(grid(second.clone()).await, (40, 12));
    // One started elsewhere is only watched.
    assert!(session.hc(&["run", "--bg", "--name", "elsewhere", "sh"]).status.success());
    assert_eq!(phone.first_mode("elsewhere", false), Mode::Watch);
    // Unless every terminal shown is taken: then one the desktop holds is
    // the phone's too.
    let desktop = screen(&session);
    let held = desktop.viewer("elsewhere", Mode::Type { cols: 100, rows: 30 }, Palette::default());
    eventually("elsewhere at the desktop's grid", async || grid("elsewhere".to_owned()).await == (100, 30)).await;
    assert_eq!(phone.first_mode("elsewhere", true), Mode::Type { cols: 40, rows: 12 });
    let shown = phone.viewer("elsewhere", phone.first_mode("elsewhere", true), Palette::default());
    eventually("elsewhere at the phone's grid", async || grid("elsewhere".to_owned()).await == (40, 12)).await;
    drop((shown, held));
    // Shown before the screen's grid is known (the app just started), an
    // eager one is taken once it is.
    let held = desktop.viewer("elsewhere", Mode::Type { cols: 100, rows: 30 }, Palette::default());
    eventually("elsewhere back at the desktop's grid", async || grid("elsewhere".to_owned()).await == (100, 30)).await;
    let early = Engine::start(connector(&session), Options::default());
    ready(&early).await;
    assert_eq!(early.first_mode("elsewhere", true), Mode::Watch);
    let shown = early.viewer("elsewhere", Mode::Watch, Palette::default());
    shown.eager();
    shown.fit(44, 14);
    eventually("elsewhere at the grid it fits", async || grid("elsewhere".to_owned()).await == (44, 14)).await;
    drop((shown, held, early));

    // The app started again: what the phone started is still its own, past
    // the core's first listing; what is gone is let go.
    let started = phone.started();
    assert_eq!(started, vec![first.clone(), second.clone()]);
    drop(viewer);
    drop(phone);
    let again = Engine::start(connector(&session), Options { size: Some((40, 12)), display: json!({"cell": [7, 16]}), ..Default::default() });
    again.adopt(started.into_iter().chain(["t99".to_owned()]));
    ready(&again).await;
    eventually("t99 let go", async || !again.started().contains(&"t99".to_owned())).await;
    assert_eq!(again.first_mode(&first, false), Mode::Type { cols: 40, rows: 12 });
    assert_eq!(again.first_mode("elsewhere", false), Mode::Watch);
}

/// Polls `ok` until it holds, failing after 10 s.
async fn eventually(what: &str, ok: impl AsyncFn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ok().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_nobody_holds_is_the_phones_until_another_screen_types() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    assert!(session.hc(&["run", "--bg", "--name", "loose", "sh"]).status.success());
    let core = phone.client().unwrap();
    let loose = core.call("terminal.get", json!({"terminal": "loose"})).await.unwrap()["address"].as_str().unwrap().to_owned();
    let grid = async || {
        let r = core.call("terminal.get", json!({"terminal": loose})).await.unwrap();
        (r["cols"].as_u64().unwrap() as u16, r["rows"].as_u64().unwrap() as u16)
    };
    assert_eq!(grid().await, (200, 120), "headless");

    // Watched with room for 50x18: nobody holds it, so the phone takes it.
    let viewer = phone.viewer(&loose, phone.first_mode(&loose, false), Palette::default());
    viewer.fit(50, 18);
    eventually("the phone to take it", async || grid().await == (50, 18)).await;
    assert_eq!(viewer.mode(), Mode::Type { cols: 50, rows: 18 });

    // The desktop types at its own grid: the phone lets go and watches it there.
    let desk = session.connect().await;
    let attached = desk.attach(herdc_wire::Attach::new(&loose, 90, 30)).await.unwrap();
    attached.stream.input(b" ");
    eventually("the desktop's grid", async || grid().await == (90, 30)).await;
    eventually("the phone to watch", async || viewer.mode() == Mode::Watch).await;
    eventually("the phone's screen at it", async || viewer.with_screen(|s| (s.cols(), s.rows())) == (90, 30)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(grid().await, (90, 30), "the phone does not take it back");

    // The desktop goes: nobody holds it again.
    drop(attached);
    drop(desk);
    eventually("the phone to take it again", async || grid().await == (50, 18)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_is_shown_as_its_screen_not_queued_byte_by_byte() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let client = phone.client().unwrap();
    let flood = "perl -e '$t=time+5; print \"y\" x 70, \"\\n\" while time<$t; print \"flood over\\n\"; sleep 60'";
    let run = client.call("terminal.run", json!({"argv": ["sh", "-c", flood]})).await.unwrap();
    let address = run["address"].as_str().unwrap().to_owned();
    let viewer = phone.viewer(&address, Mode::Watch, Palette::default());
    let started = tokio::time::Instant::now();
    // Byte by byte, the core's 4 MiB backlog at the pace would take 8 s more.
    shows(&viewer, "the end of the flood", |v| v.snapshot().text().contains("flood over")).await;
    let took = started.elapsed();
    assert!(took < Duration::from_secs(8), "the end showed after {took:?}");
    client.call("terminal.kill", json!({"terminal": address})).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_sight_a_viewer_takes_nothing_and_back_shows_the_screen() {
    let session = Arc::new(Session::new());
    let phone = screen(&session);
    ready(&phone).await;
    let client = phone.client().unwrap();
    let run = client.call("terminal.run", json!({"argv": ["cat"]})).await.unwrap();
    let address = run["address"].as_str().unwrap().to_owned();
    let viewer = phone.viewer(&address, Mode::Watch, Palette::default());
    shows(&viewer, "the attach", |v| v.info().attached).await;

    phone.set_shown(false);
    shows(&viewer, "letting go", |v| !v.info().attached).await;
    client.call("terminal.send", json!({"terminal": address, "text": "said while away", "force": true})).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!viewer.snapshot().text().contains("said while away"));

    phone.set_shown(true);
    shows(&viewer, "what was said, back", |v| v.snapshot().text().contains("said while away")).await;
    client.call("terminal.kill", json!({"terminal": address})).await.unwrap();
}
