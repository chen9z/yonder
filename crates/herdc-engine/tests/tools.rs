//! The built-in mods' work against a real repo, through an isolated core's
//! `machine.exec`.

use herdc_engine::tools::{agents, changes, files, machines, worktree, There};
use herdc_testkit::Session;
use std::path::Path;
use std::process::Command;

fn git(session: &Session, cwd: &Path, args: &[&str]) {
    let out = session.command("git").current_dir(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A repo in the session's home: a.txt committed, then changed; b.txt new.
fn repo(session: &Session) -> String {
    let root = session.path("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(session, &root, &["init", "-q", "-b", "main"]);
    git(session, &root, &["config", "user.name", "t"]);
    git(session, &root, &["config", "user.email", "t@t"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(root.join("src/.hidden"), "").unwrap();
    git(session, &root, &["add", "-A"]);
    git(session, &root, &["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("a.txt"), "one\n2\nthree\n").unwrap();
    std::fs::write(root.join("b.txt"), "new\n").unwrap();
    // The core reports folders as git does (on macOS /var is /private/var).
    let out = Command::new("git").current_dir(&root).args(["rev-parse", "--show-toplevel"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn files_changes_and_worktrees_of_a_repo() {
    let session = Session::new();
    let root = repo(&session);
    let there = There::new(session.connect().await, None);

    // Files: folders first, hidden ones too, not .git; marks by full path.
    let listing = files::list(&there, &root).await.unwrap();
    let names: Vec<_> = listing.entries.iter().map(|e| (e.name.as_str(), e.dir)).collect();
    assert_eq!(names, [("src", true), ("a.txt", false), ("b.txt", false)]);
    assert_eq!(files::list(&there, &format!("{root}/src")).await.unwrap().entries[0].name, ".hidden");
    assert!(files::list(&there, &format!("{root}/nope")).await.is_err());
    let marks = files::marks(&there, &root).await;
    assert_eq!(marks.get(&format!("{root}/a.txt")), Some(&'M'));
    assert_eq!(marks.get(&format!("{root}/b.txt")), Some(&'A'));
    assert_eq!(files::preview(&there, &format!("{root}/b.txt")).await, files::Preview::Text { text: "new\n".into(), cut: false });
    assert_eq!(files::preview(&there, &format!("{root}/gone")).await, files::Preview::Gone);

    // Changes: counted, diffed, committed.
    let found = changes::scan(&there, &format!("{root}/src")).await.unwrap();
    assert_eq!(found.root, root);
    assert!(found.head);
    let paths: Vec<_> = found.files.iter().map(|f| (f.path.as_str(), f.status, f.add, f.del)).collect();
    assert_eq!(paths, [("a.txt", 'M', 1, 1), ("b.txt", 'A', 0, 0)]);
    let diff = changes::diff(&there, &found, &found.files[0]).await;
    let changed: Vec<_> = diff.iter().filter(|l| matches!(l.kind, changes::Kind::Add | changes::Kind::Del)).map(|l| (l.number, l.text.as_str())).collect();
    assert_eq!(changed, [(Some(2), "two"), (Some(2), "2")]);
    let fresh = changes::diff(&there, &found, &found.files[1]).await;
    assert_eq!(fresh.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["@@ -0,0 +1,1 @@", "new"]);
    assert!(changes::commit(&there, &root, "  ").await.is_err());
    changes::commit(&there, &root, "second").await.unwrap();
    assert!(changes::scan(&there, &root).await.unwrap().files.is_empty());
    assert_eq!(changes::scan(&there, &session.home().display().to_string()).await.err().as_deref(), Some("Not a git repository"));

    // Worktrees: made under ~/.herdr/worktrees, listed, removed.
    let repo = worktree::repo_of(&there, &root).await.unwrap().unwrap();
    assert!(!repo.linked);
    let home = there.home().await.unwrap();
    let made = worktree::create(&there, &repo, &home, "feat/phone").await.unwrap();
    assert!(made.ends_with("/.herdr/worktrees/proj/feat-phone"), "{made}");
    let all = worktree::checkouts(&there, &repo).await.unwrap();
    assert_eq!(all.iter().map(|c| (c.branch.as_deref(), c.main)).collect::<Vec<_>>(), [(Some("main"), true), (Some("feat/phone"), false)]);
    let linked = worktree::repo_of(&there, &made).await.unwrap().unwrap();
    assert!(linked.linked);
    assert_eq!(linked.main, repo.main);
    std::fs::write(Path::new(&made).join("wip"), "x").unwrap();
    let risk = worktree::risk(&there, &linked.top).await.unwrap();
    assert!(risk.dirty && risk.unpushed.is_empty());
    worktree::remove(&there, &linked, &risk).await.unwrap();
    assert!(!Path::new(&made).exists());
    assert_eq!(worktree::checkouts(&there, &repo).await.unwrap().len(), 1);

    // Agents: herdr's whole list, none of them on this scratch PATH but sh.
    let known = agents::known(&there).await.unwrap();
    assert_eq!(known.first().map(|k| k.key()), Some("claude"));
    assert!(known.iter().all(|k| !k.name().is_empty() && k.run().is_array()));
}

#[tokio::test(flavor = "multi_thread")]
async fn ssh_config_hosts_to_pick_from_includes_too() {
    let session = Session::new();
    let ssh = session.path(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*\nHost vps box\n  HostName 10.0.0.7\n  User can\nHost *.lan !x\nMatch all\n  User nobody\n").unwrap();
    std::fs::write(ssh.join("config.d/work"), "Host work=x\nHost work\n  HostName=work.example\nHost vps\n").unwrap();
    let there = There::new(session.connect().await, None);
    let home = there.home().await.unwrap();
    // The included `Host vps` comes first, so the main file's leaves `box` alone (as the desktop's).
    let hosts = machines::hosts(&there, &home).await;
    let shown: Vec<_> = hosts.iter().map(|h| (h.alias.as_str(), h.aliases.clone(), h.hostname.as_deref(), h.user.as_deref())).collect();
    assert_eq!(
        shown,
        [("work=x", vec![], None, None), ("work", vec![], Some("work.example"), None), ("vps", vec![], None, None), ("box", vec![], Some("10.0.0.7"), Some("can"))]
    );
    // Nothing to reach: said, not hung.
    assert!(matches!(machines::add(&there, "nobody@127.0.0.1:1").await, machines::Added::No(_) | machines::Added::NeedsYou(_)));
    assert!(machines::added(&there).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_session_is_found_by_its_label_or_folder_and_read_as_it_grows() {
    use herdc_engine::tools::transcript::{self, Said, Transcript};
    let session = Session::new();
    let there = There::new(session.connect().await, None);
    let folder = session.home().join(".pi/agent/sessions/--work-nova--");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("2026-11-05T17-59-33-091Z_01a1-b3.jsonl");
    let user = |id: &str, parent: &str, text: &str| format!(r#"{{"type":"message","id":"{id}","parentId":"{parent}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#);
    let mut lines: Vec<String> = vec![r#"{"type":"session","id":"s"}"#.into()];
    for i in 1..=transcript::TAIL + 5 {
        lines.push(user(&format!("m{i}"), &format!("m{}", i - 1), &format!("n{i}")));
    }
    // An older session there, then this one.
    std::fs::write(folder.join("2026-01-01T00-00-00-000Z_old.jsonl"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
    let path = file.to_string_lossy().into_owned();

    // By the label, else the newest in its folder; none elsewhere.
    assert_eq!(transcript::find(&there, Some("pi:id:01a1-b3"), "/nowhere").await.unwrap().as_deref(), Some(path.as_str()));
    assert_eq!(transcript::find(&there, None, "/work/nova").await.unwrap().as_deref(), Some(path.as_str()));
    assert_eq!(transcript::find(&there, Some("pi:id:gone"), "/work/elsewhere").await.unwrap(), None);

    // The end of it first; then what was added, a line half written held back.
    let mut t = Transcript::default();
    t.take(&transcript::read(&there, &path, 0).await.unwrap());
    assert!(t.cut());
    assert_eq!(t.read, transcript::TAIL + 6);
    assert_eq!(t.lines().len(), transcript::TAIL);
    let mut more = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
    use std::io::Write;
    write!(more, "{}\n{{\"type\":\"mess", user("x", &format!("m{}", transcript::TAIL + 5), "later")).unwrap();
    drop(more);
    let got = transcript::read(&there, &path, t.read).await.unwrap();
    t.take(&got);
    assert_eq!(t.read, transcript::TAIL + 7);
    assert_eq!(t.lines().last().map(|l| &l.said), Some(&Said::You { text: "later".into(), images: 0 }));
    // What came before, asked for: all of it now.
    t.take(&transcript::earlier(&there, &path, t.start).await.unwrap());
    assert!(!t.cut());
    assert_eq!(t.lines().len(), transcript::TAIL + 6);
    assert_eq!(t.lines()[0].said, Said::You { text: "n1".into(), images: 0 });

    // A picture put beside a terminal lands on its machine.
    let ran = there.core.call("terminal.run", serde_json::json!({"argv": ["cat"]})).await.unwrap();
    let address = ran["address"].as_str().unwrap().to_owned();
    let landed = transcript::put(&there, &address, "shot.png", b"\x89PNG").await.unwrap();
    assert!(landed.ends_with("shot.png"), "{landed}");
    assert_eq!(std::fs::read(&landed).unwrap(), b"\x89PNG");
    there.core.call("terminal.kill", serde_json::json!({"terminal": address})).await.unwrap();
}

/// Going back in a real Pi's session by its keys, against the Pi it was
/// written for (`pi` on the PATH; `--ignored`): no model asked, the
/// session made up. The branch it leaves is where the next entry goes.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_real_pi_goes_back_to_before_one_of_your_messages() {
    use herdc_engine::tools::transcript;
    use serde_json::json;
    let session = Session::new();
    let there = There::new(session.connect().await, None);
    let dir = session.path("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("probe.jsonl");
    let at = |n: usize| format!("2026-11-15T01:00:{n:02}.000Z");
    let user = |n, id: &str, parent: Option<&str>, text: &str| json!({"type": "message", "id": id, "parentId": parent, "timestamp": at(n), "message": {"role": "user", "content": [{"type": "text", "text": text}], "timestamp": 1}});
    let reply = |n, id: &str, parent: &str, text: &str| json!({"type": "message", "id": id, "parentId": parent, "timestamp": at(n), "message": {"role": "assistant", "content": [{"type": "text", "text": text}], "api": "x", "provider": "x", "model": "x", "stopReason": "stop", "timestamp": 1,
        "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}}}});
    // A branch left (the same words as one on the branch shown), then
    // the branch shown: alpha, beta, gamma.
    let lines = [
        json!({"type": "session", "version": 3, "id": "0aaa0000-0000-7000-8000-000000000001", "timestamp": at(0), "cwd": dir}),
        user(1, "u1", None, "alpha one"), reply(2, "a1", "u1", "reply one"),
        user(3, "u2b", Some("a1"), "beta two"), reply(4, "a2b", "u2b", "reply old"),
        user(5, "u2", Some("a1"), "beta two"), reply(6, "a2", "u2", "reply two"),
        user(7, "u3", Some("a2"), "gamma three"), reply(8, "a3", "u3", "reply three"),
    ];
    std::fs::write(&file, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let f = file.to_str().unwrap();
    let ran = there.core.call("terminal.run", json!({"argv": ["pi", "--session", f, "--session-dir", dir], "cwd": dir})).await.unwrap();
    let t = ran["address"].as_str().unwrap().to_owned();
    let screen = || async { there.core.call("terminal.read", json!({"terminal": t})).await.unwrap()["text"].as_str().unwrap_or_default().to_owned() };
    let shown = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let s = screen().await;
            if s.contains("gamma three") && s.contains("reply three") {
                break s;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }).await.unwrap();
    // Not one of its messages: nothing chosen, its tree closed again.
    assert!(transcript::rewind(&there, &t, "delta four", 0).await.is_err());
    assert!(!screen().await.contains("[user]"), "{shown}");
    // "beta two", one of yours after it: the one on the branch shown.
    transcript::rewind(&there, &t, "beta two", 1).await.unwrap();
    // The next entry (a command, asking no model) goes on from its parent.
    transcript::send(&there, &t, "/name went back").await.unwrap();
    let last = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let text = std::fs::read_to_string(&file).unwrap();
            let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
            if last["type"] == "session_info" {
                break last;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }).await.unwrap();
    assert_eq!(last["parentId"], "a1", "{}", screen().await);
    there.core.call("terminal.kill", json!({"terminal": t})).await.unwrap();
}
