use super::*;

    #[tokio::test]
    async fn reconnect_discards_old_completion_as_cancelled() {
        let (_session, tasks) = crate::tasks::Session::new();
        let (jobs, mut queue) = mpsc::unbounded_channel();
        let (view, _) = watch::channel(Arc::new(View::default()));
        let (notes, _) = broadcast::channel(1);
        let (clients, _) = watch::channel(None);
        let mut state = State::new(jobs, Outlets { view, notes, clients }, None, tasks);
        let (tx, rx) = oneshot::channel();
        let mut reply = crate::commands::Reply::new(tx);
        state.later(async {}, move |_, _| reply.send(Ok(json!({"ok":true}))));
        state.generation += 1;
        let job = tokio::time::timeout(Duration::from_secs(2), queue.recv()).await.unwrap().unwrap();
        job(&mut state);
        assert!(rx.await.unwrap().unwrap_err().contains("cancelled"));
    }

    #[test]
    fn records_show_their_word_program_and_plain_title() {
        let r = normalize(json!({"address": "t1", "status": {"state": "working", "app": "claude-code"}, "title": "\u{2819} fix", "argv": ["zsh"]}));
        assert_eq!(r["status"], "working");
        assert_eq!(r["program"]["key"], "claude");
        assert_eq!(r["report"]["app"], "claude-code");
        assert_eq!(r["title"], "fix");
        let shell = normalize(json!({"address": "t2", "argv": ["/bin/zsh"], "foreground": {"argv": ["vim", "x"]}}));
        assert_eq!(shell["program"]["key"], "vim");
        assert!(shell["status"].is_null());
    }

    #[test]
    fn folders_are_asked_again_only_when_the_answer_could_change() {
        let last = FolderCheck { at: Instant::now(), cwd: "/a".into(), busy: true, repo: true };
        assert!(recheck(None, "/a", false, true));
        assert!(recheck(Some(&last), "/b", true, true));
        assert!(recheck(Some(&last), "/a", false, true));
        assert!(!recheck(Some(&last), "/a", true, true));
    }

    #[test]
    fn early_launch_failures_show_exit_code_and_last_output_lines() {
        let mut record = json!({"reason":"exited", "exit_code":127, "created_ms":1000, "finished_ms":3000});
        assert_eq!(launch_failure(&record, "old\none\ntwo\nthree\nfour\nfive\n\n").as_deref(), Some("Exit code 127\none\ntwo\nthree\nfour\nfive"));
        assert_eq!(launch_failure(&record, "").as_deref(), Some("Exit code 127"));
        record["exit_code"] = 0.into();
        assert!(launch_failure(&record, "").is_none());
        record["exit_code"] = 1.into();
        record["finished_ms"] = 3001.into();
        assert!(launch_failure(&record, "").is_none());
        record["finished_ms"] = 2000.into();
        record["reason"] = "killed".into();
        assert!(launch_failure(&record, "").is_none());
        assert!(launch_failure(&Value::Null, "").is_none());
    }
