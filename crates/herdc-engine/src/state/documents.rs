//! Layout synchronization orchestration. Slot owns its state transitions.
use super::*;

impl State {
    /// This screen's own choices (focus, zoom, settled tabs, what was seen),
    /// kept across restarts like herdr-app's `view.json`: looked at once a
    /// second at most (settles come every half second, and in floods), or
    /// `now` (the engine stopping).
    pub(crate) fn save_view(&mut self, now: bool) {
        let Some(path) = self.store.clone() else { return };
        if !self.documents.values().any(|s| s.loaded()) {
            return;
        }
        if !now && self.view_looked.is_some_and(|at| at.elapsed() < VIEW_EVERY) {
            return;
        }
        self.view_looked = Some(Instant::now());
        let mut view = mirror::View::capture(&self.model);
        if let Some(saved) = &self.initial_view {
            let documents = &self.documents;
            view.preserve_unloaded(saved, |machine| documents.get(machine.unwrap_or("")).is_some_and(|s| s.loaded()));
        }
        let text = serde_json::to_string_pretty(&view).unwrap_or_default() + "\n";
        if text == self.written_view {
            return;
        }
        let tmp = path.with_extension("tmp");
        let wrote = path.parent().is_none_or(|d| std::fs::create_dir_all(d).is_ok())
            && std::fs::write(&tmp, &text).is_ok()
            && std::fs::rename(&tmp, &path).is_ok();
        if wrote {
            self.written_view = text;
        }
    }


    // ---- layout documents --------------------------------------------------

    pub(crate) fn flush_documents(&mut self, refresh: bool) {
        if !self.watching {
            return;
        }
        let Some(core) = self.core.clone() else { return };
        for (machine, slot) in &mut self.documents {
            let doc = mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
            let Some(work) = slot.prepare(machine, doc, refresh) else { continue };
            let jobs = self.jobs.clone();
            let generation = self.generation;
            let core = core.clone();
            self.tasks.spawn(async move {
                let (machine, outcome) = docs::round(core, work).await;
                let _ = jobs.send(Box::new(move |s: &mut State| {
                    if s.generation != generation {
                        return;
                    }
                    s.documents_updated(None, vec![(machine, outcome)]);
                }));
            });
        }
    }

    fn layout_notice(&self, title: &str, body: &str) {
        self.note(Note::info(title, body));
    }

    pub(super) fn documents_updated(&mut self, machines: Option<Value>, results: Vec<(String, Outcome)>) -> bool {
        let mut changed = false;
        for (machine, result) in results {
            let Some(slot) = self.documents.get_mut(&machine) else { continue };
            let on = (!machine.is_empty()).then_some(machine.as_str());
            let projection = mirror::Document::capture(&self.model, on);
            let applied = slot.apply(result, &projection);
            if let Some(document) = applied.replace {
                document.replace(&mut self.model, on);
                if applied.first && let Some(view) = &self.initial_view {
                    view.apply_machine(&mut self.model, on);
                }
                changed = true;
            }
            if let Some(error) = applied.notice {
                tracing::warn!("layout on {machine:?} unavailable: {error}");
                let place = if machine.is_empty() { "This machine" } else { machine.as_str() };
                self.layout_notice("Layout unavailable", &format!("{place}: {error}"));
            }
            if applied.conflict {
                self.layout_notice("Layout changed elsewhere", "The other screen's layout is shown.");
            }
        }
        if let Some(machines) = machines {
            // Temporary machines are agents' short-lived sandboxes: left out.
            let kept: Vec<&Value> = machines["machines"].as_array().into_iter().flatten().filter(|m| m["temporary"] != true).collect();
            let states: HashMap<&str, &str> = kept.iter().filter_map(|m| Some((m["name"].as_str()?, m["state"].as_str()?))).collect();
            for machine in &kept {
                let Some(name) = machine["name"].as_str() else { continue };
                let state = machine["state"].as_str().unwrap_or("unavailable");
                let reason = (state != "connected").then(|| {
                    machine["why"].as_str().map(str::to_owned).unwrap_or_else(|| match state {
                        "needs_update" => format!("{name} needs a core update; run hc machine update {name}"),
                        "needs_login" => format!("{name} needs login; run hc machine login {name}"),
                        _ => format!("{name} is {state}"),
                    })
                });
                self.documents.entry(name.to_owned()).or_default().connection(state == "connected", machine["since_ms"].as_u64(), reason);
            }
            if self.model.spaces.iter().any(|s| s.machine.as_deref().is_some_and(|m| !states.contains_key(m))) {
                let mut view = mirror::View::capture(&self.model);
                if let Some(saved) = &self.initial_view {
                    let documents = &self.documents;
                    view.preserve_unloaded(saved, |m| documents.get(m.unwrap_or("")).is_some_and(|s| s.loaded()));
                }
                self.initial_view = Some(view);
            }
            self.documents.retain(|m, _| m.is_empty() || states.contains_key(m.as_str()));
            let before = self.model.spaces.len();
            self.model.spaces.retain(|s| s.machine.as_deref().is_none_or(|m| states.contains_key(m)));
            changed |= before != self.model.spaces.len();
            self.machines = kept
                .iter()
                .filter_map(|m| {
                    let name = m["name"].as_str()?;
                    Some(Machine {
                        name: Some(name.to_owned()),
                        state: m["state"].as_str().unwrap_or("unavailable").to_owned(),
                        layout: self.documents.get(name).map_or("unavailable", Slot::layout),
                        error: self.documents.get(name).and_then(|s| s.error()),
                        host: m["host"].as_str().map(str::to_owned),
                    })
                })
                .collect();
        }
        if changed {
            self.tend_terminals(false);
        }
        changed
    }

    /// The machine a space's terminals run on must have its layout in hand
    /// before anything there changes.
    pub(crate) fn require_document(&self, machine: Option<&str>) -> Result<(), String> {
        match self.documents.get(machine.unwrap_or("")) {
            Some(slot) if slot.available() => Ok(()),
            Some(slot) => Err(slot.error().unwrap_or_else(|| "This machine's layout is unavailable. Try again when it reconnects.".into())),
            None => Err(match machine {
                Some(m) => format!("Herdr has no machine called {m}. See hc machine ls; machines added with --temp stay out of Herdr."),
                None => "This machine's layout is unavailable. Try again when it reconnects.".into(),
            }),
        }
    }

}
