//! Against a real, isolated herdr-core.

use herdc_testkit::{eventually, Session};
use herdc_wire::{compat, Attach, Item, Terminals};
use serde_json::json;
use std::time::Duration;

async fn read_until(stream: &herdc_wire::Stream, needle: &str) -> String {
    let mut seen = Vec::new();
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(item) = stream.next().await {
            if let Item::Data(bytes) = item {
                // Only what just came (and what a needle across it needs) is new.
                let from = seen.len().saturating_sub(needle.len());
                seen.extend(bytes);
                if String::from_utf8_lossy(&seen[from..]).contains(needle) {
                    return true;
                }
            }
        }
        false
    })
    .await;
    let text = String::from_utf8_lossy(&seen).into_owned();
    assert!(matches!(found, Ok(true)), "never saw {needle:?} in {text:?}");
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_runs_attaches_types_and_detaches() {
    let session = Session::new();
    let core = session.connect().await;
    compat::check(core.hello()).expect("this core serves herdc");

    let run = core
        .call("terminal.run", json!({"argv": ["cat"], "labels": {"herdc.test": "1"}, "size": {"cols": 80, "rows": 24}}))
        .await
        .unwrap();
    let address = run["address"].as_str().unwrap().to_owned();

    let attached = core.attach(Attach::new(&address, 80, 24)).await.unwrap();
    assert_eq!(attached.terminal["address"], address);
    attached.stream.input(b"ping from herdc\r");
    read_until(&attached.stream, "ping from herdc").await;

    // Errors keep the core's kind.
    let missing = core.call("terminal.get", json!({"terminal": "t999"})).await.unwrap_err();
    assert_eq!(missing.kind(), Some("not_found"));

    core.call("terminal.kill", json!({"terminal": address})).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn events_follow_terminals_and_documents() {
    let session = Session::new();
    let core = session.connect().await;
    let (events, snapshot) = core.subscribe(json!({})).await.unwrap();
    let mut terminals = Terminals::new(&snapshot);

    let run = core.call("terminal.run", json!({"argv": ["sleep", "30"]})).await.unwrap();
    let address = run["address"].as_str().unwrap().to_owned();
    let doc = core.call("doc.set", json!({"name": "herdr-app", "version": 0, "value": {"spaces": []}})).await.unwrap();
    assert_eq!(doc["version"], 1);

    let mut saw_doc = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while terminals.get(&address).is_none() || !saw_doc {
        assert!(tokio::time::Instant::now() < deadline, "events never arrived");
        if let Ok(Some(Item::Data(bytes))) = tokio::time::timeout(Duration::from_secs(1), events.next()).await {
            let pushed = terminals.push(&bytes);
            saw_doc |= pushed.docs;
        }
    }

    // A stale version is a conflict, not an overwrite.
    let stale = core.call("doc.set", json!({"name": "herdr-app", "version": 0, "value": {}})).await.unwrap_err();
    assert_eq!(stale.kind(), Some("conflict"));

    core.call("terminal.kill", json!({"terminal": address})).await.unwrap();
    eventually("the kill to arrive", Duration::from_secs(10), || {
        while let Some(Item::Data(bytes)) = events.try_next() {
            terminals.push(&bytes);
        }
        terminals.get(&address).is_some_and(|t| t["state"] == "finished")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_pipe_fails_calls_and_ends_streams() {
    let session = Session::new();
    let core = session.connect().await;
    let run = core.call("terminal.run", json!({"argv": ["sleep", "30"]})).await.unwrap();
    let attached = core.attach(Attach::new(run["address"].as_str().unwrap(), 80, 24)).await.unwrap();
    session.hc(&["server", "stop"]);
    tokio::time::timeout(Duration::from_secs(10), core.closed()).await.expect("the pipe ends with the core");
    let mut last = None;
    while let Some(item) = attached.stream.next().await {
        last = Some(item);
    }
    assert!(matches!(last, Some(Item::Lost | Item::Closed(_))));
    assert!(matches!(core.call("terminal.list", json!({})).await, Err(herdc_wire::Error::Lost)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paced_stream_falls_behind_a_flood_and_says_so() {
    let session = Session::new();
    let core = session.connect().await;
    let flood = "perl -e '$t=time+20; print \"y\" x 70, \"\\n\" while time<$t'";
    let run = core.call("terminal.run", json!({"argv": ["sh", "-c", flood], "size": {"cols": 80, "rows": 24}})).await.unwrap();
    let mut attach = Attach::new(run["address"].as_str().unwrap(), 80, 24);
    attach.pace = Some(128 * 1024);
    let attached = core.attach(attach).await.unwrap();
    let started = std::time::Instant::now();
    let mut read = 0;
    while !attached.stream.behind() {
        assert!(started.elapsed() < Duration::from_secs(10), "never behind, {read} bytes read");
        if let Ok(Some(Item::Data(bytes))) = tokio::time::timeout(Duration::from_secs(1), attached.stream.next()).await {
            read += bytes.len();
        }
    }
    // The window, then the pace.
    let most = 256.0 * 1024.0 + started.elapsed().as_secs_f64() * 128.0 * 1024.0 + 16.0 * 1024.0;
    assert!((read as f64) <= most, "{read} bytes in {:.1} s", started.elapsed().as_secs_f64());
    core.call("terminal.kill", json!({"terminal": run["address"]})).await.unwrap();
}
