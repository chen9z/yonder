//! The main agent: the one the person talks to, which hands the work to
//! others (the agents it starts, its sub-agents). One per computer, on a
//! tab of its own: the conversation with it ([`crate::chat`], Pi's), else
//! its terminal; a field under it to write to it, and the agents working
//! for it over it.

use crate::agents;
use crate::home::{self, Home};
use crate::live::Live;
use crate::nav::{self, Route};
use crate::prefs::Prefs;
use crate::{theme, ui};
use herdc_engine::AgentView;
use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The main agent as kept: its terminal, and where it runs and what, to
/// find it again once its terminal is another (a restart takes a new
/// address).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lead {
    pub address: String,
    pub space: String,
    pub space_name: String,
    pub app: Option<String>,
}

impl Lead {
    pub fn of(a: &AgentView) -> Lead {
        Lead { address: a.address.clone(), space: a.space.clone(), space_name: a.space_name.clone(), app: a.app.clone() }
    }

    /// The agent it is now: its terminal, else the same program in its
    /// space (by its id, else its name).
    pub fn find<'v>(&self, agents: &'v [AgentView]) -> Option<&'v AgentView> {
        let same = |a: &&AgentView| a.app == self.app;
        agents
            .iter()
            .find(|a| a.address == self.address)
            .or_else(|| agents.iter().filter(same).find(|a| a.space == self.space))
            .or_else(|| agents.iter().filter(same).find(|a| !self.space_name.is_empty() && a.space_name == self.space_name))
    }

    /// What it is called: its space's name, else its program's.
    pub fn name(&self) -> String {
        if self.space_name.is_empty() { self.app.clone().unwrap_or_default() } else { self.space_name.clone() }
    }
}

/// The main agent of the computer shown, as kept.
fn kept(live: &Live) -> Memo<Option<Lead>> {
    let prefs = use_context::<Prefs>().expect("prefs");
    let computer = live.computer.id();
    computed(move || prefs.lead(&computer))
}

/// The main agent of the computer shown, where it runs now.
pub fn found(live: &Live) -> Memo<Option<AgentView>> {
    let (kept, world) = (kept(live), live.view);
    computed(move || kept.with(|k| k.as_ref().and_then(|k| world.with(|v| k.find(&v.agents).cloned()))))
}

/// Makes `agent` the main one (`None`: none), and shows it.
pub fn set(prefs: Prefs, live: &Live, agent: Option<&AgentView>) {
    let computer = live.computer.id();
    prefs.leads.update(|l| match agent {
        Some(a) => { l.insert(computer, Lead::of(a)); }
        None => { l.remove(&computer); }
    });
    if agent.is_some() {
        nav::truncate(0);
        ui::smoothly();
        home::shown().set(Home::Lead);
    }
}

/// Offers the agents to choose the main one from (none, too, once one is).
pub fn choose(prefs: Prefs, live: Live) {
    let computer = live.computer.id();
    let now = prefs.lead(&computer);
    let agents = live.view.with_untracked(|v| v.agents.clone());
    let current = now.as_ref().and_then(|n| n.find(&agents)).map(|a| a.address.clone());
    spawn(async move {
        use crate::pick::{self, Icon, Item};
        let mut items: Vec<Item> = agents
            .iter()
            .map(|a| {
                let program = a.program["name"].as_str().or(a.app.as_deref()).unwrap_or_default();
                let named = [a.space_name.as_str(), program].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                Item::new(a.address.clone(), named, Icon::Program(a.program.clone())).checked(current.as_deref() == Some(&a.address))
            })
            .collect();
        if now.is_some() {
            items.push(Item::glyph("none", t!("No main agent"), "x"));
        }
        match pick::one(t!("Main agent"), items).await.as_deref() {
            Some("none") => set(prefs, &live, None),
            Some(address) => {
                let agent = agents.iter().find(|a| a.address == address).cloned();
                set(prefs, &live, agent.as_ref());
            }
            None => {}
        }
    });
}

/// What it and the agents working for it ask of the person: how many
/// need them; else whether it said something not read yet (done).
pub fn waiting(live: &Live) -> Memo<(usize, &'static str)> {
    let (found, world) = (found(live), live.view);
    computed(move || {
        let Some(lead) = found.get() else { return (0, "blocked") };
        let team = world.with(|v| agents::listed_subs(&v.terminals, &v.agents, &lead.address));
        match (lead.status == "blocked") as usize + team.iter().filter(|s| s.status == Some("blocked")).count() {
            0 => ((lead.status == "done") as usize, "done"),
            blocked => (blocked, "blocked"),
        }
    })
}

/// The home page's title on its tab: the main agent, how it is, and a tap
/// to choose another. The computer's name while there is none.
pub fn title(live: Live) -> impl IntoElement {
    let found = found(&live);
    let prefs = use_context::<Prefs>().expect("prefs");
    let computer = live.computer.id();
    let name = live.computer.name.clone();
    // Drawn again for another agent, not for each turn of this one.
    let who = computed(move || found.with(|f| f.as_ref().map(|a| (a.space.clone(), a.space_name.clone()))));
    dynamic(move || match who.get() {
        None => text(name.clone()).flex_1().pl(px(12.0)).text_size(theme::TITLE).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).number_of_lines(1).into_element(),
        Some((space, space_name)) => {
            let color = prefs.colors.with_untracked(|c| c.get(&format!("{computer}\u{0}{space}")).copied()).unwrap_or(0);
            let program = move || found.with(|f| f.as_ref().map(|a| a.program.clone()).unwrap_or_default());
            let called = space_name.clone();
            let live = live.clone();
            view()
                .flex_1()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .pl(px(12.0))
                .h(px(48.0))
                .role(AccessibilityRole::Button)
                .label(t!("Main agent: {}", called))
                .active(|s| s.opacity(0.6))
                .on_press(move || choose(prefs, live.clone()))
                .child(ui::space_badge(move || space_name.clone(), move || color, program))
                .child(
                    view()
                        .flex_shrink(1.0)
                        .gap(px(1.0))
                        .child(text(called).text_size(theme::HEADLINE).font_weight(FontWeight::BOLD).text_color(theme::FOREGROUND).number_of_lines(1))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(text(move || found.with(|f| f.as_ref().map(|a| a.program["name"].as_str().or(a.app.as_deref()).unwrap_or_default().to_owned()).unwrap_or_default())).text_size(theme::CAPTION).text_color(theme::MUTED))
                                .child(ui::status_mark(move || found.with(|f| f.as_ref().map(|a| a.status)), || false)),
                        ),
                )
                .child(ui::glyph("chevron-down", 14.0, theme::DIM))
                .into_element()
        }
    })
}

/// Whether the main agent shows as a conversation now: read so, and
/// not turned to its terminal.
pub fn talking(live: &Live) -> Memo<bool> {
    let found = found(live);
    let readable = computed(move || found.with(|f| f.as_ref().is_some_and(crate::chat::readable)));
    computed(move || readable.get() && crate::chat::talking().get().0)
}

/// The main agent's tab: the agents working for it, the conversation (or
/// its terminal), and what the person says to it. Chosen and gone, it
/// says so.
pub fn lead_page(live: Live) -> impl IntoElement {
    let (kept, found) = (kept(&live), found(&live));
    let address = computed(move || found.with(|f| f.as_ref().map(|a| a.address.clone())));
    let talking = talking(&live);
    dynamic(move || match (address.get(), kept.get()) {
        (Some(address), _) => {
            let name = kept.with_untracked(|k| k.as_ref().map(Lead::name).unwrap_or_default());
            let shown = live.clone();
            view()
                .flex_1()
                .child({
                    let (live, address) = (live.clone(), address.clone());
                    when(move || !crate::chat::reading().get().0, move || team(live.clone(), address.clone()))
                })
                .child(dynamic(move || {
                    if talking.get() {
                        crate::chat::chat(shown.clone(), address.clone(), name.clone()).into_element()
                    } else {
                        crate::terminal::talk(shown.engine.clone(), address.clone(), name.clone()).into_element()
                    }
                }))
                .into_element()
        }
        (None, Some(lead)) => empty(live.clone(), t!("{} isn't running", lead.name()), t!("Its space is kept. Start it there again, or choose another.")).into_element(),
        (None, None) => empty(live.clone(), t!("Your main agent").into(), t!("Choose the agent you talk to. It shows here, ready for what you say, with the agents it hands work to above it.")).into_element(),
    })
}

/// No main agent to show: why, and the way to choose one.
fn empty(live: Live, title: String, said: &'static str) -> impl IntoElement {
    let prefs = use_context::<Prefs>().expect("prefs");
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .px(px(32.0))
        .gap(px(12.0))
        .child(ui::glyph("lead", 40.0, theme::DIM))
        .child(text(title).text_size(theme::HEADLINE).font_weight(FontWeight::SEMIBOLD).text_color(theme::FOREGROUND).text_center())
        .child(text(said).text_size(theme::CALLOUT).line_height(px(20.0)).text_color(theme::MUTED).text_center())
        .child(view().h(px(8.0)))
        .child(ui::button(t!("Choose an agent"), move || choose(prefs, live.clone())).test_id("choose-lead"))
}

/// The agents working for it (those it started, theirs too), each a tap
/// from its terminal; nothing while there are none.
fn team(live: Live, address: String) -> impl IntoElement {
    let world = live.view;
    let subs = computed(move || world.with(|v| agents::listed_subs(&v.terminals, &v.agents, &address)));
    when(move || subs.with(|s| !s.is_empty()), move || {
        let live = live.clone();
        scroll()
            .horizontal()
            .scrollbar(false)
            .flex_grow(0.0)
            .child(
                view().flex_row().gap(px(8.0)).px(px(12.0)).pt(px(2.0)).pb(px(8.0)).child(each(
                    move || subs.get(),
                    |s| s.address.clone(),
                    move |s, _| member(live.clone(), s),
                )),
            )
    })
}

/// One agent working for it: what runs, its name, how it is.
fn member(live: Live, s: ReadSignal<agents::Sub>) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .pl(px(10.0))
        .pr(px(8.0))
        .rounded(px(16.0))
        .bg(theme::RAISED)
        .border_1()
        .border_color(theme::BORDER)
        .active(|st| st.scale(ui::PRESSED))
        .transition()
        .duration(ui::QUICK)
        .role(AccessibilityRole::Button)
        .label(move || s.with(|s| t!("{}, sub-agent", s.name)))
        .on_press(move || {
            let address = s.with_untracked(|s| s.address.clone());
            if live.view.with_untracked(|v| v.agents.iter().any(|a| a.address == address)) {
                live.run("agent.focus", json!({ "terminal": address }));
                nav::workspace();
            } else {
                nav::push(Route::Peek(address));
            }
        })
        .child(ui::program_icon(move || s.with(|s| s.program.clone()), 14.0))
        .child(text(move || s.with(|s| s.name.clone())).max_w(px(140.0)).text_size(theme::CALLOUT).text_color(theme::FOREGROUND).number_of_lines(1))
        .child(ui::status_mark(move || s.with(|s| s.status), || true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(address: &str, space: &str, name: &str, app: &str) -> AgentView {
        serde_json::from_value(json!({ "address": address, "pane": "p", "space": space, "space_name": name, "machine": null,
            "tab": "tab", "tab_index": 1, "tab_name": "1", "status": "idle", "app": app, "program": {},
            "title": "", "seq": 1, "pinned": false, "focused": false })).unwrap()
    }

    #[test]
    fn a_main_agent_restarted_is_found_again_in_its_space() {
        let lead = Lead::of(&agent("t1q", "sp1", "nova", "pi"));
        // As it was.
        let now = [agent("t17", "sp2", "kg", "claude"), agent("t1q", "sp1", "nova", "pi")];
        assert_eq!(lead.find(&now).map(|a| a.address.as_str()), Some("t1q"));
        // Restarted: another terminal, the same program in the same space.
        let now = [agent("t2a", "sp1", "nova", "codex"), agent("t2b", "sp1", "nova", "pi")];
        assert_eq!(lead.find(&now).map(|a| a.address.as_str()), Some("t2b"));
        // Its space made again: by its name.
        let now = [agent("t3", "sp9", "nova", "pi")];
        assert_eq!(lead.find(&now).map(|a| a.address.as_str()), Some("t3"));
        // Another program there is not it.
        assert_eq!(lead.find(&[agent("t4", "sp1", "nova", "codex")]), None);
    }
}
