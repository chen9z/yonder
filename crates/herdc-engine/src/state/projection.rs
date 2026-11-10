//! Pure projection of the current model into a published view.
use super::*;

impl State {
    pub(super) fn program(&self, addr: &str) -> Value {
        self.live.get(addr).map(|r| r["program"].clone()).unwrap_or(Value::Null)
    }

    pub(super) fn title(&self, addr: &str) -> String {
        self.live.get(addr).and_then(|r| r["title"].as_str()).unwrap_or_default().to_owned()
    }

    /// The agent an agents page shows: the chosen one while it lives, else
    /// the focused pane's if it's an agent, else the first agent.
    pub(crate) fn shown_agent(&self) -> Option<String> {
        let focused = self.model.focused_pane().and_then(|p| self.model.address_of(&p));
        self.agent
            .clone()
            .filter(|a| self.live.contains_key(a))
            .or_else(|| focused.filter(|a| self.model.status(a).is_some()))
            .or_else(|| self.agent_order().into_iter().next())
    }

    pub(crate) fn agent_order(&self) -> Vec<String> {
        self.model.all_addresses().into_iter().filter(|a| self.model.status(a).is_some()).collect()
    }

    pub(super) fn view(&self) -> View {
        let m = &self.model;
        let home = &self.home;
        let focused_pane = m.focused_pane();
        let mut agents = Vec::new();
        let spaces = m
            .spaces
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let active_tab = p.active_tab().map(|c| c.id.clone());
                let first = p.active_tab().and_then(|c| c.focused_pane()).and_then(|pane| m.address_of(&pane));
                let tabs = p
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(ci, c)| {
                        let panes = c
                            .root
                            .panes()
                            .into_iter()
                            .map(|n| {
                                let focused = m.active.as_deref() == Some(p.id.as_str()) && focused_pane.as_deref() == Some(n.id.as_str());
                                let zoomed = c.zoomed.as_deref() == Some(n.id.as_str());
                                let addr = n.terminal().map(str::to_owned);
                                let (mod_name, props) = match n.content() {
                                    Some(Content::Mod { name, props }) => (Some(name.clone()), props.clone()),
                                    _ => (None, Value::Null),
                                };
                                let a = addr.as_deref().and_then(|a| m.agents.get(a));
                                if let Some(addr) = &addr
                                    && let Some(status) = m.status(addr)
                                {
                                    agents.push(AgentView {
                                        address: addr.clone(),
                                        pane: n.id.clone(),
                                        space: p.id.clone(),
                                        space_name: m.space_name(p, home),
                                        machine: p.machine.clone(),
                                        tab: c.id.clone(),
                                        tab_index: ci + 1,
                                        tab_name: Model::tab_name(p, c),
                                        status,
                                        app: a.and_then(|a| a.app.clone()),
                                        program: self.program(addr),
                                        title: self.title(addr),
                                        seq: a.map_or(0, |a| a.seq),
                                        // The core's, else when this engine saw it change.
                                        since: self.live.get(addr).and_then(|r| r["status_ms"].as_u64()).unwrap_or(a.map_or(0, |a| a.since)),
                                        pinned: self.live.get(addr).and_then(|r| r.pointer("/labels/herdr.pinned")).and_then(Value::as_str).is_some_and(|v| !v.is_empty()),
                                        focused,
                                    });
                                }
                                PaneView {
                                    pane: n.id.clone(),
                                    title: addr.as_deref().map(|a| self.title(a)).or_else(|| mod_name.clone()).unwrap_or_default(),
                                    cwd: addr.as_deref().and_then(|a| self.cwds.get(a).cloned()),
                                    branch: addr.as_deref().and_then(|a| m.folder_info.get(a)).and_then(|f| f.branch.clone()),
                                    status: addr.as_deref().and_then(|a| m.status(a)),
                                    app: a.and_then(|a| a.app.clone()),
                                    program: addr.as_deref().map(|a| self.program(a)).unwrap_or(Value::Null),
                                    live: addr.as_deref().is_some_and(|a| self.live.contains_key(a)),
                                    shown: self.swapped.get(&n.id).cloned(),
                                    terminal: addr,
                                    mod_name,
                                    props,
                                    focused,
                                    zoomed,
                                }
                            })
                            .collect();
                        TabView {
                            id: c.id.clone(),
                            index: ci + 1,
                            name: Model::tab_name(p, c),
                            named: !c.name.is_empty(),
                            active: active_tab.as_ref() == Some(&c.id),
                            status: m.rollup(&model::addresses(&c.root)),
                            settled: c.settled.is_some(),
                            settled_at: c.settled,
                            changed_at: m.changed_at(c),
                            working_since: m.working_since(c),
                            focused: c.focused_pane(),
                            zoomed: c.zoomed.clone(),
                            root: c.root.clone(),
                            panes,
                        }
                    })
                    .collect();
                let all: Vec<String> = p.tabs.iter().flat_map(|c| model::addresses(&c.root)).collect();
                SpaceView {
                    id: p.id.clone(),
                    index: i + 1,
                    name: m.space_name(p, home),
                    named: !p.name.is_empty(),
                    folder: p.folder.clone(),
                    machine: p.machine.clone(),
                    branch: first.and_then(|a| m.folder_info.get(&a)).and_then(|f| f.branch.clone()),
                    active: m.active.as_deref() == Some(p.id.as_str()),
                    status: m.rollup(&all),
                    tabs,
                }
            })
            .collect();
        let focused = m.active_space().and_then(|p| {
            let tab = p.active_tab()?;
            let pane = tab.focused_pane();
            let terminal = pane.as_deref().and_then(|p| m.address_of(p));
            Some(Focused {
                space: p.id.clone(),
                tab: tab.id.clone(),
                shown: pane.as_deref().and_then(|p| self.swapped.get(p).cloned()),
                cwd: terminal.as_deref().and_then(|a| self.cwds.get(a).cloned()),
                agent: self.shown_agent(),
                terminal,
                pane,
            })
        });
        let local = self.documents.get("");
        let mut machines = vec![Machine {
            name: None,
            state: if self.core.is_some() { "connected" } else { "connecting" }.into(),
            layout: local.map_or("unavailable", Slot::layout),
            error: local.and_then(|s| s.error()),
            host: match &self.link {
                Link::Connected { core, .. } => Some(core.clone()),
                _ => None,
            },
        }];
        machines.extend(self.machines.iter().cloned().map(|mut mc| {
            if let Some(slot) = mc.name.as_deref().and_then(|n| self.documents.get(n)) {
                mc.layout = slot.layout();
                mc.error = slot.error();
            }
            mc
        }));
        View { link: self.link.clone(), home: self.home.clone(), machines, spaces, agents, terminals: self.records.clone(), focused }
    }
}
