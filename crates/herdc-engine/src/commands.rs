//! The commands that change the world, as herdr-app defines them
//! (`src/commands.rs`) and runs them (`src/window.rs`, `dispatch`). Those
//! that only change a screen (the palette, settings, a terminal's find bar
//! or scroll) are the screen's own.
//!
//! Where the desktop asks the person in a dialog (a name, closing a space),
//! the reply says what to ask instead: `{"ask": {"kind": "prompt" |
//! "confirm", ..., "command": {"name", "args"}}}`; run that command with
//! the answer.

use crate::geometry::{self, Rect};
use crate::state::{Spawn, State, TerminalChange};
use herdc_model::layout::{Axis, Content, Node, Size};
use herdc_model::model::{self, Tab};
use herdc_model::shell;
use serde_json::{json, Value};
use tokio::sync::oneshot;

pub struct Arg {
    pub name: &'static str,
    pub required: bool,
}

pub struct Command {
    pub name: &'static str,
    pub args: &'static [Arg],
    pub about: &'static str,
}

const fn req(name: &'static str) -> Arg {
    Arg { name, required: true }
}

const fn opt(name: &'static str) -> Arg {
    Arg { name, required: false }
}

const fn cmd(name: &'static str, args: &'static [Arg], about: &'static str) -> Command {
    Command { name, args, about }
}

pub const COMMANDS: &[Command] = &[
    // What exists.
    cmd("space.new", &[opt("name"), opt("folder"), opt("tab"), opt("argv"), opt("task"), opt("machine")],
        "Make a space: its name (default: none, so it's called after where its first terminal is), the folder its terminals start in (default: the focused terminal's), its first tab's name, optionally an agent (argv) and its first message (task), and the machine it lives on (default: the shown space's; empty string: the core's own)."),
    cmd("space.close", &[opt("id"), opt("confirmed")], "Close a space and end its terminals (asks first unless confirmed)."),
    cmd("space.rename", &[opt("id"), opt("name")], "Rename a space (asks when no name; an empty name: called after where its first terminal is again)."),
    cmd("space.move", &[req("id"), req("index")], "Move a space to a position (1 = first)."),
    cmd("tab.new", &[opt("name"), opt("space"), opt("folder"), opt("argv"), opt("task"), opt("mod"), opt("props")],
        "Make a tab in a space (default: the shown one) with a terminal in folder (default: the focused terminal's), optionally an agent (argv) and its first message (task); or with a mod as its only pane (`mod`, its `props`). With no space open, a new tab starts a space."),
    cmd("tab.close", &[opt("id"), opt("confirmed")], "Close a tab and end its terminals."),
    cmd("tab.rename", &[opt("id"), opt("name")], "Rename a tab (asks when no name; an empty name: called by its number again)."),
    cmd("tab.move", &[req("id"), req("index")], "Move a tab to a position in its space (1 = first)."),
    cmd("pane.split", &[req("direction"), opt("pane"), opt("argv"), opt("mod"), opt("props")], "Split a pane (default: the focused one) \"right\" or \"down\" with a new terminal, or with a mod (`mod`, its `props`) beside it."),
    cmd("pane.close", &[opt("pane")], "Close a pane and end its terminal."),
    cmd("terminal.open", &[opt("argv"), opt("folder"), opt("place"), opt("name")], "Start a terminal: as a new tab (place \"tab\", default), or \"right\"/\"down\" of the focused pane."),
    cmd("terminal.keep", &[req("terminal"), opt("place")], "Give a running terminal without a pane (a sub-agent) a pane: a new tab, or \"right\"/\"down\"."),
    cmd("terminal.park", &[req("terminal")], "Take a terminal's pane away and disable core keep; it keeps running (a sub-agent shows under its parent again)."),
    cmd("terminal.right_click", &[req("to"), opt("terminal")], "Where right-clicks in a terminal (default: the focused one) go when its program takes the mouse: \"program\" or \"menu\"."),
    cmd("terminal.pin", &[req("terminal"), req("pinned")], "Pin an agent to the top of the agent lists (pinned true) or unpin it (false); every Herdr screen shows it."),
    cmd("terminal.kill", &[req("terminal")], "End a terminal and its sub-agents."),
    // What you see.
    cmd("space.select", &[opt("index"), opt("id")], "Show a space (by position or id)."),
    cmd("space.next", &[], "Show the next space."),
    cmd("space.prev", &[], "Show the previous space."),
    cmd("tab.select", &[opt("index"), opt("id")], "Show a tab (by position in the space, or id)."),
    cmd("tab.settle", &[opt("id")], "Put a tab away on this screen (default: the shown one): out of the lists, still running. It comes back when shown or when an agent in it needs you."),
    cmd("tab.unsettle", &[opt("id")], "Bring a settled tab back to the lists."),
    cmd("tab.next", &[], "Show the next tab."),
    cmd("tab.prev", &[], "Show the previous tab."),
    cmd("pane.zoom", &[opt("pane")], "Show only this pane of its tab, or all again."),
    cmd("pane.focus", &[opt("pane"), opt("terminal")], "Focus a pane (or the pane showing a terminal)."),
    cmd("pane.go", &[req("direction")], "Focus the pane to the \"left\", \"right\", \"up\" or \"down\"."),
    cmd("pane.swap", &[opt("direction"), opt("pane"), opt("target")], "Swap a pane (default: the focused one) with its neighbor in a direction, or with another pane of its tab (target)."),
    cmd("terminal.show", &[opt("terminal"), opt("pane")], "Show a terminal in a pane for now, in place of its own (no terminal: back)."),
    cmd("agent.focus", &[req("terminal")], "Show an agent (sub-agents too)."),
    cmd("agent.next", &[], "Show the next agent."),
    cmd("agent.prev", &[], "Show the previous agent."),
    cmd("layout.resize", &[req("id"), opt("px"), opt("weight")], "Size a pane or split of a tab."),
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// A key's `name:value` shorthand: the value fills the first argument (a
/// number fills `index` when there is one).
pub fn shorthand(c: &Command, value: &str) -> Value {
    let index = value.parse::<u64>().ok().filter(|_| c.args.iter().any(|a| a.name == "index"));
    match (index, c.args.first()) {
        (Some(i), _) => json!({ "index": i }),
        (None, Some(a)) => json!({ a.name: value }),
        (None, None) => Value::Null,
    }
}

/// Rejects misspelled arguments before a command can use its current-target default.
pub fn validate_args(c: &Command, args: &Value) -> Result<(), String> {
    let valid = if c.args.is_empty() { "none".into() } else { c.args.iter().map(|a| a.name).collect::<Vec<_>>().join(", ") };
    match args {
        Value::Null => Ok(()),
        Value::Object(args) => {
            let unknown: Vec<_> = args.keys().filter(|key| !c.args.iter().any(|a| a.name == key.as_str())).map(String::as_str).collect();
            if !unknown.is_empty() {
                return Err(format!("{}: unknown argument(s): {}; valid arguments: {valid}", c.name, unknown.join(", ")));
            }
            for (key, value) in args {
                if matches!(key.as_str(), "id" | "pane" | "space" | "terminal" | "target") && !value.is_string() && !value.is_null() {
                    return Err(format!("{}: argument {key} must be a string (or null to omit it)", c.name));
                }
            }
            Ok(())
        }
        _ => Err(format!("{}: arguments must be an object; valid arguments: {valid}", c.name)),
    }
}

/// The first required argument that's missing (an empty string counts as missing).
pub fn missing(c: &Command, args: &Value) -> Option<&'static str> {
    c.args.iter().filter(|a| a.required).map(|a| a.name).find(|k| match args.get(k) {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        _ => false,
    })
}

/// Where a command's answer goes. Dropped unanswered, it says `ok`.
pub(crate) struct Reply(Option<oneshot::Sender<Result<Value, String>>>);

impl Reply {
    pub(crate) fn new(tx: oneshot::Sender<Result<Value, String>>) -> Reply {
        Reply(Some(tx))
    }

    pub(crate) fn send(mut self, result: Result<Value, String>) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(result);
        }
    }

    fn value(self, v: Value) {
        self.send(Ok(v))
    }

    fn fail(self, e: impl Into<String>) {
        self.send(Err(e.into()))
    }

    fn ask(self, ask: Value) {
        self.value(json!({ "ask": ask }))
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(Ok(json!({ "ok": true })));
        }
    }
}

fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn owned(a: &Value, k: &str) -> Option<String> {
    arg(a, k).map(str::to_owned)
}

fn argv(a: &Value) -> Option<Vec<String>> {
    a.get("argv").and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// An agent with its first message, in that machine's shell, which stays
/// when the agent exits.
fn agent_argv(argv: Option<Vec<String>>, task: Option<&str>) -> Option<Vec<String>> {
    let task = task.map(str::trim).filter(|t| !t.is_empty());
    argv.filter(|a| !a.is_empty()).map(|mut cmd| {
        cmd.extend(task.map(str::to_owned));
        shell::launch(cmd)
    })
}

fn machine_of_address(addr: &str) -> Option<&str> {
    addr.split_once('/').map(|(m, _)| m)
}

impl State {
    /// Runs a command for the person: what it made, or why not.
    pub(crate) fn run(&mut self, name: &str, mut args: Value, reply: Reply) {
        // `name:value`, as in keys.json.
        if let Some((n, v)) = name.split_once(':')
            && let Some(c) = find(n)
        {
            if let Err(error) = validate_args(c, &args) {
                return reply.fail(error);
            }
            let mut short = shorthand(c, v);
            if let (Some(s), Some(extra)) = (short.as_object_mut(), args.as_object()) {
                s.extend(extra.clone());
            }
            return self.run(n, short, reply);
        }
        let Some(command) = find(name) else {
            return reply.fail(format!("unknown command {name:?}"));
        };
        if let Err(error) = validate_args(command, &args) {
            return reply.fail(error);
        }
        if name.ends_with(".rename") && args.get("name").is_none_or(Value::is_null) {
            return self.ask_name(name, args, reply);
        }
        if let Some(arg) = missing(command, &args) {
            return reply.fail(format!("{name} needs {arg}"));
        }
        if !args.is_object() {
            args = json!({});
        }
        if name == "space.new" {
            let machine = arg(&args, "machine").map(str::to_owned).or_else(|| self.model.active_space().and_then(|s| s.machine.clone()));
            args["machine"] = json!(machine.filter(|m| !m.is_empty()));
        }
        let structural = matches!(
            name,
            "space.new" | "space.close" | "space.rename" | "space.move" | "tab.new" | "tab.close" | "tab.rename" | "tab.move" | "pane.split"
                | "pane.close" | "pane.swap" | "terminal.keep" | "terminal.park" | "layout.resize"
        );
        if structural {
            let node = ["id", "space", "pane"].into_iter().find_map(|k| arg(&args, k));
            let target = node.and_then(|id| self.model.space(id).or_else(|| self.model.space_of(id))).or_else(|| self.model.active_space());
            let machine = match name {
                "space.new" => arg(&args, "machine"),
                "terminal.park" => arg(&args, "terminal").and_then(machine_of_address),
                _ => target.and_then(|s| s.machine.as_deref()),
            };
            if let Err(error) = self.require_document(machine) {
                return reply.fail(error);
            }
        }
        self.dispatch(name, &args, reply);
    }

    /// Asks for a new name (space.rename, tab.rename), starting with the one
    /// someone gave (empty for an automatic one).
    fn ask_name(&mut self, name: &str, mut args: Value, reply: Reply) {
        if !args.is_object() {
            args = json!({});
        }
        let (title, hint, current) = if name == "space.rename" {
            let id = owned(&args, "id").or_else(|| self.model.active.clone());
            let current = id.as_deref().and_then(|i| self.model.space(i)).map(|p| p.name.clone());
            args["id"] = json!(id);
            ("Rename space", "Empty: named after its folder", current)
        } else {
            let id = owned(&args, "id").or_else(|| self.model.active_tab().map(|c| c.id.clone()));
            let current = id.as_deref().and_then(|i| self.model.tab(i)).map(|c| c.name.clone());
            args["id"] = json!(id);
            ("Rename tab", "Empty: its number", current)
        };
        reply.ask(json!({ "kind": "prompt", "title": title, "hint": hint, "value": current.unwrap_or_default(), "field": "name",
            "command": { "name": name, "args": args } }));
    }

    fn dispatch(&mut self, name: &str, a: &Value, reply: Reply) {
        let active_tab = self.model.active_tab().map(|c| c.id.clone());
        match name {
            // ---- spaces
            "space.new" => {
                let request = NewSpace {
                    name: arg(a, "name").unwrap_or_default().trim().to_owned(),
                    folder: owned(a, "folder"),
                    tab: arg(a, "tab").unwrap_or_default().trim().to_owned(),
                    argv: agent_argv(argv(a), arg(a, "task")).or(argv(a)),
                    machine: owned(a, "machine").filter(|m| !m.is_empty()),
                };
                self.new_space(request, reply);
            }
            "space.close" => {
                let Some(id) = owned(a, "id").or_else(|| self.model.active.clone()) else { return reply.fail("no space to close") };
                let Some(p) = self.model.space(&id) else { return reply.fail(format!("no space {id:?}")) };
                if a["confirmed"] != true {
                    let panes: usize = p.tabs.iter().map(|c| c.root.panes().len()).sum();
                    let count = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
                    let detail = format!("{} \u{2014} {}, {}", self.model.space_name(p, &self.home), count(p.tabs.len(), "tab"), count(panes, "terminal"));
                    return reply.ask(json!({ "kind": "confirm", "title": "Close space?", "detail": detail, "action": "Close",
                        "command": { "name": "space.close", "args": { "id": id, "confirmed": true } } }));
                }
                let addrs = self.model.remove_space(&id);
                self.kill(&addrs);
            }
            "space.rename" => {
                let Some(id) = owned(a, "id").or_else(|| self.model.active.clone()) else { return reply.fail("no space to rename") };
                let Some(name) = arg(a, "name") else { return reply.fail("space.rename needs name (empty: automatic)") };
                let Some(p) = self.model.space_mut(&id) else { return reply.fail(format!("no space {id:?}")) };
                p.name = name.trim().to_owned();
            }
            "space.move" => {
                let (Some(id), Some(i)) = (arg(a, "id"), a["index"].as_u64()) else { return reply.fail("space.move needs id and index") };
                if !self.model.move_space(id, i as usize) {
                    reply.fail(format!("no space {id:?}"))
                }
            }
            "space.select" => {
                let id = owned(a, "id").or_else(|| {
                    let i = a["index"].as_u64()? as usize;
                    self.model.spaces.get(i.checked_sub(1)?).map(|p| p.id.clone())
                });
                match id.filter(|id| self.model.space(id).is_some()) {
                    Some(id) => self.model.active = Some(id),
                    None => reply.fail("no such space"),
                }
            }
            "space.next" | "space.prev" => {
                let n = self.model.spaces.len();
                if let Some(i) = self.model.active_index().filter(|_| n > 0) {
                    let j = if name == "space.next" { (i + 1) % n } else { (i + n - 1) % n };
                    self.model.active = Some(self.model.spaces[j].id.clone());
                }
            }
            // ---- tabs
            "tab.new" => {
                let name = arg(a, "name").unwrap_or_default().trim().to_owned();
                match owned(a, "mod") {
                    Some(m) => {
                        let r = self.new_mod_tab(owned(a, "space"), &name, m, a.get("props").cloned().unwrap_or(Value::Null));
                        reply.send(r);
                    }
                    // A new tab with no space open starts one, as space.new does.
                    None if a.get("space").is_none_or(Value::is_null) && self.model.active_space().is_none() => {
                        let request = NewSpace {
                            name: String::new(),
                            folder: owned(a, "folder"),
                            tab: name,
                            argv: agent_argv(argv(a), arg(a, "task")).or(argv(a)),
                            machine: None,
                        };
                        self.new_space(request, reply);
                    }
                    None => {
                        let argv = agent_argv(argv(a), arg(a, "task")).or(argv(a));
                        self.new_tab(owned(a, "space"), name, owned(a, "folder"), argv, reply);
                    }
                }
            }
            "tab.close" => {
                let Some(id) = owned(a, "id").or(active_tab) else { return reply.fail("no tab to close") };
                let Some(p) = self.model.space_of(&id) else { return reply.fail(format!("no tab {id:?}")) };
                // The last tab takes its space with it: asked like closing the space.
                if p.tabs.len() == 1 {
                    let args = json!({ "id": p.id, "confirmed": a.get("confirmed").cloned().unwrap_or(Value::Null) });
                    return self.dispatch("space.close", &args, reply);
                }
                let (addrs, _) = self.model.remove_tab(&id);
                self.kill(&addrs);
            }
            "tab.rename" => {
                let Some(id) = owned(a, "id").or(active_tab) else { return reply.fail("no tab to rename") };
                let Some(name) = arg(a, "name") else { return reply.fail("tab.rename needs name (empty: its number)") };
                let Some(c) = self.model.tab_mut(&id) else { return reply.fail(format!("no tab {id:?}")) };
                c.name = name.trim().to_owned();
            }
            "tab.move" => {
                let (Some(id), Some(i)) = (arg(a, "id"), a["index"].as_u64()) else { return reply.fail("tab.move needs id and index") };
                let Some(pid) = self.model.space_of(id).map(|p| p.id.clone()) else { return reply.fail(format!("no tab {id:?}")) };
                let p = self.model.space_mut(&pid).unwrap();
                let from = p.tabs.iter().position(|c| c.id == id).unwrap();
                let c = p.tabs.remove(from);
                let to = (i as usize).saturating_sub(1).min(p.tabs.len());
                p.tabs.insert(to, c);
            }
            "tab.settle" | "tab.unsettle" => {
                let Some(id) = owned(a, "id").or(active_tab) else { return reply.fail("no tab") };
                let found = if name == "tab.settle" {
                    self.model.settle(&id, model::now_ms())
                } else {
                    self.model.tab(&id).is_some() && {
                        self.model.unsettle(&id);
                        true
                    }
                };
                if !found {
                    reply.fail(format!("no tab {id:?}"))
                }
            }
            "tab.select" => {
                // By position among the tabs the lists show (settled ones are put away).
                let id = owned(a, "id").or_else(|| {
                    let i = a["index"].as_u64()? as usize;
                    self.model.active_space()?.tabs.iter().filter(|c| c.settled.is_none()).nth(i.checked_sub(1)?).map(|c| c.id.clone())
                });
                if !id.is_some_and(|id| self.model.select_tab(&id)) {
                    reply.fail("no such tab")
                }
            }
            "tab.next" | "tab.prev" => {
                if let Some(p) = self.model.active_space()
                    && let Some(i) = active_tab.as_ref().and_then(|c| p.tabs.iter().position(|x| x.id == *c))
                {
                    let n = p.tabs.len();
                    let step = |k: usize| if name == "tab.next" { (i + k) % n } else { (i + n * k - k) % n };
                    if let Some(id) = (1..n).map(|k| &p.tabs[step(k)]).find(|c| c.settled.is_none()).map(|c| c.id.clone()) {
                        self.model.select_tab(&id);
                    }
                }
            }
            // ---- panes
            "pane.split" => {
                let axis = match arg(a, "direction") {
                    Some("right") => Axis::Row,
                    Some("down") => Axis::Column,
                    other => return reply.fail(format!("pane.split direction is \"right\" or \"down\", not {other:?}")),
                };
                match owned(a, "mod") {
                    Some(m) => {
                        let r = self.split_mod(owned(a, "pane"), axis, m, a.get("props").cloned().unwrap_or(Value::Null));
                        reply.send(r);
                    }
                    None => self.split(owned(a, "pane"), axis, argv(a), None, reply),
                }
            }
            "pane.close" => {
                let Some(pane) = owned(a, "pane").or_else(|| self.model.focused_pane()) else { return reply.fail("no pane to close") };
                let Some(c) = self.model.tab_of(&pane) else { return reply.fail(format!("no pane {pane:?}")) };
                if c.root.find(&pane).is_some_and(|n| !n.closable()) {
                    return;
                }
                if c.root.panes().len() == 1 {
                    let id = c.id.clone();
                    return self.dispatch("tab.close", &json!({ "id": id }), reply);
                }
                let addr = self.model.address_of(&pane);
                self.model.remove_pane(&pane);
                self.kill(&addr.into_iter().collect::<Vec<_>>());
            }
            "pane.zoom" => {
                let pane = owned(a, "pane").or_else(|| self.model.focused_pane());
                if let Some(pane) = pane
                    && let Some(tab) = self.model.tab_of(&pane).map(|c| c.id.clone())
                    && let Some(c) = self.model.tab_mut(&tab)
                    && c.root.panes().len() > 1
                {
                    c.zoomed = if c.zoomed.as_deref() == Some(pane.as_str()) { None } else { Some(pane) };
                }
            }
            "pane.focus" => {
                let pane = owned(a, "pane").or_else(|| arg(a, "terminal").and_then(|t| self.model.pane_of(t)));
                let Some(p) = pane.filter(|p| self.model.tab_of(p).is_some()) else { return reply.fail("no such pane") };
                self.model.focus_pane(&p);
                self.agent = self.model.address_of(&p);
                self.swapped.remove(&p);
            }
            "pane.go" => {
                let dir = arg(a, "direction").unwrap_or_default();
                if !matches!(dir, "left" | "right" | "up" | "down") {
                    return reply.fail(format!("{name} direction is left, right, up or down, not {dir:?}"));
                }
                if let Some(from) = self.model.focused_pane()
                    && let Some(to) = self.neighbor(&from, dir, true)
                {
                    self.model.focus_pane(&to);
                }
            }
            "pane.swap" => {
                let explicit = arg(a, "pane").is_some();
                let Some(pane) = owned(a, "pane").or_else(|| self.model.focused_pane()) else {
                    if arg(a, "target").is_some() {
                        reply.fail("no pane to swap");
                    }
                    return;
                };
                let r = match (arg(a, "direction"), arg(a, "target")) {
                    (Some(_), Some(_)) => Err("pane.swap takes direction or target, not both".into()),
                    (None, None) => Err("pane.swap needs direction or target".into()),
                    (None, Some(target)) => self.model.swap_panes(&pane, target),
                    (Some(dir @ ("left" | "right" | "up" | "down")), None) => self.swap_with_neighbor(&pane, explicit, dir),
                    (Some(dir), None) => Err(format!("pane.swap direction is left, right, up or down, not {dir:?}")),
                };
                if let Err(e) = r {
                    reply.fail(e)
                }
            }
            // ---- terminals
            "terminal.open" => {
                let argv = argv(a);
                match arg(a, "place").unwrap_or("tab") {
                    "right" => self.split(None, Axis::Row, argv, owned(a, "folder"), reply),
                    "down" => self.split(None, Axis::Column, argv, owned(a, "folder"), reply),
                    _ => {
                        let name = owned(a, "name")
                            .or_else(|| argv.as_ref().and_then(|v| v.first()).map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()))
                            .unwrap_or_else(|| "shell".into());
                        self.new_tab(None, name, owned(a, "folder"), argv, reply);
                    }
                }
            }
            "terminal.keep" => {
                let r = self.keep_terminal(arg(a, "terminal").unwrap_or_default(), arg(a, "place").unwrap_or("tab"));
                reply.send(r);
            }
            "terminal.park" => {
                let t = arg(a, "terminal").unwrap_or_default().to_owned();
                if self.changing.contains(&t) {
                    return reply.fail("This terminal already has a pane change in progress.");
                }
                let r = self.change_terminal(t.clone(), TerminalChange::Park).map(|_| json!({ "pending": true, "terminal": t }));
                reply.send(r);
            }
            "terminal.right_click" => {
                let to = match arg(a, "to") {
                    Some("program") => "program",
                    Some("menu") => "",
                    other => return reply.fail(format!("terminal.right_click to is \"program\" or \"menu\", not {other:?}")),
                };
                let t = owned(a, "terminal").or_else(|| self.model.focused_pane().and_then(|p| self.model.address_of(&p)));
                let Some(t) = t.filter(|t| self.live.contains_key(t)) else { return reply.fail("no such running terminal") };
                self.label_now(&t, json!({ "herdr.right-click": to }));
            }
            "terminal.pin" => {
                let Some(pinned) = a["pinned"].as_bool() else { return reply.fail("terminal.pin pinned is true or false") };
                let t = arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return reply.fail(format!("no running terminal {t:?}"));
                }
                self.label_now(&t, json!({ "herdr.pinned": if pinned { "1" } else { "" } }));
            }
            "terminal.kill" => {
                let t = arg(a, "terminal").unwrap_or_default();
                if !self.live.contains_key(t) {
                    return reply.fail(format!("no running terminal {t:?}"));
                }
                self.kill(&[t.to_owned()]);
            }
            "terminal.show" => {
                let pane = owned(a, "pane").or_else(|| self.model.focused_pane());
                let target = arg(a, "terminal").filter(|t| self.live.contains_key(*t)).map(str::to_owned);
                if let Some(p) = pane {
                    match target {
                        // One with a pane of its own is shown there.
                        Some(t) if let Some(own) = self.model.pane_of(&t) => {
                            self.swapped.remove(&p);
                            self.model.focus_pane(&own);
                        }
                        Some(t) => {
                            self.swapped.insert(p.clone(), t);
                            self.model.focus_pane(&p);
                        }
                        None => {
                            self.swapped.remove(&p);
                        }
                    }
                }
            }
            // ---- agents
            "agent.focus" => {
                let t = arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return reply.fail(format!("no running terminal {t:?}"));
                }
                self.focus_agent(&t);
            }
            "agent.next" | "agent.prev" => {
                let order = self.agent_order();
                if order.is_empty() {
                    return;
                }
                let current = self.shown_agent().map(|a| self.root_agent(&a));
                let n = order.len();
                let next = match current.and_then(|c| order.iter().position(|a| *a == c)) {
                    Some(i) if name == "agent.next" => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                    None if name == "agent.next" => 0,
                    None => n - 1,
                };
                let addr = order[next].clone();
                self.focus_agent(&addr);
            }
            // ---- layout
            "layout.resize" => {
                let id = arg(a, "id").unwrap_or_default();
                let Some(n) = self.model.spaces.iter_mut().flat_map(|p| p.tabs.iter_mut()).find_map(|c| c.root.find_mut(id)) else {
                    return reply.fail(format!("no node {id:?}"));
                };
                if let Some(px) = a["px"].as_f64() {
                    n.size = Some(Size::Px { px: px as f32 });
                } else if let Some(w) = a["weight"].as_f64() {
                    n.size = Some(Size::Weight { weight: w as f32 });
                }
            }
            other => reply.fail(format!("command {other:?} is listed but not handled")),
        }
    }

    fn label_now(&self, terminal: &str, labels: Value) {
        let Ok(core) = self.client() else { return };
        let params = json!({ "terminal": terminal, "labels": labels });
        tokio::spawn(async move {
            if let Err(e) = core.call("terminal.label", params).await {
                tracing::warn!("label: {e}");
            }
        });
    }

    /// Makes `addr` the shown agent; a pane showing it also becomes the focused pane.
    fn focus_agent(&mut self, addr: &str) {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
        }
        self.agent = Some(addr.to_owned());
    }

    /// The pane nearest `from` in `dir`, in the shown tab.
    fn neighbor(&self, from: &str, dir: &str, use_history: bool) -> Option<String> {
        let tab = self.model.active_tab()?;
        let rects = geometry::rects(tab.shown(), Rect::SQUARE);
        let cur = rects.iter().find(|(id, _)| id == from)?.1;
        let candidates = rects.into_iter().filter(|(id, _)| id != from);
        geometry::nearest(cur, dir, candidates, if use_history { &self.pane_history } else { &[] })
    }

    fn swap_with_neighbor(&mut self, from: &str, explicit: bool, dir: &str) -> Result<(), String> {
        let model = &self.model;
        if explicit && model.tab_of(from).and_then(|c| c.root.find(from)).is_none_or(|n| !n.is_pane()) {
            return Err(format!("no pane {from:?}"));
        }
        let shown = model.active_tab().is_some_and(|c| c.shown().visible_blocks().iter().any(|n| n.id == from));
        if explicit && !shown {
            return Err(format!("pane.swap direction needs a pane in the shown tab, not {from:?}"));
        }
        if let Some(to) = self.neighbor(from, dir, false)
            && let Some(tab) = self.model.tab_of(from).map(|c| c.id.clone())
            && let Some(c) = self.model.tab_mut(&tab)
        {
            c.root.swap(from, &to);
        }
        Ok(())
    }

    /// The folder new terminals in `space` start in: where its shown tab's
    /// focused pane is now, else the folder the space was made in.
    fn folder_for(&self, space: &str) -> String {
        let p = self.model.space(space);
        let here = p.and_then(|p| p.active_tab()).and_then(|c| c.focused_pane()).and_then(|f| self.model.address_of(&f)).and_then(|a| self.cwds.get(&a).cloned());
        here.or_else(|| p.map(|p| p.folder.clone())).unwrap_or_else(|| self.home.clone())
    }

    /// A space with its first tab and terminal, shown once it runs. Its
    /// terminals start where the focused one is (on that machine), else in
    /// the machine's home.
    fn new_space(&mut self, r: NewSpace, reply: Reply) {
        let on = r.machine.clone();
        let follow = r.folder.is_none();
        let here = self
            .model
            .focused_pane()
            .and_then(|p| self.model.address_of(&p))
            .filter(|a| machine_of_address(a) == on.as_deref())
            .and_then(|a| self.cwds.get(&a).cloned());
        let folder = r.folder.or(here).unwrap_or_else(|| if on.is_some() { String::new() } else { self.home.clone() });
        let (space, tab, pane) = (self.model.fresh_id_on("sp", on.as_deref()), self.model.fresh_id_on("tb", on.as_deref()), self.model.fresh_id_on("p", on.as_deref()));
        let request = Spawn { argv: r.argv, cwd: folder, follow, space: space.clone(), tab: tab.clone(), on: on.clone() };
        self.spawn(request, move |s, result| match result {
            Ok((addr, folder)) => {
                s.model.add_space(&space, &r.name, &folder, Tab::new(&tab, &r.tab, &pane, &addr));
                if let Some(p) = s.model.space_mut(&space) {
                    p.machine = on;
                }
                reply.value(json!({ "space": space, "tab": tab, "pane": pane, "terminal": addr }));
            }
            Err(e) => reply.fail(e),
        });
    }

    /// A tab in a space (default: the shown one), shown once it runs.
    fn new_tab(&mut self, space: Option<String>, name: String, folder: Option<String>, argv: Option<Vec<String>>, reply: Reply) {
        let Some(space) = space.or_else(|| self.model.active_space().map(|p| p.id.clone())) else {
            return reply.fail("no space yet: make one first (space.new)");
        };
        if self.model.space(&space).is_none() {
            return reply.fail(format!("no space {space:?}"));
        }
        let follow = folder.is_none();
        let folder = folder.unwrap_or_else(|| self.folder_for(&space));
        let on = self.model.space(&space).and_then(|p| p.machine.clone());
        let (tab, pane) = (self.model.fresh_id_on("tb", on.as_deref()), self.model.fresh_id_on("p", on.as_deref()));
        let request = Spawn { argv, cwd: folder, follow, space: space.clone(), tab: tab.clone(), on };
        self.spawn(request, move |s, result| match result {
            Ok((addr, _)) if s.model.add_tab(&space, Tab::new(&tab, &name, &pane, &addr)) => {
                s.model.active = Some(space.clone());
                reply.value(json!({ "space": space, "tab": tab, "pane": pane, "terminal": addr }));
            }
            // Its space closed meanwhile.
            Ok((addr, _)) => {
                s.kill(&[addr]);
                reply.fail(format!("no space {space:?}"));
            }
            Err(e) => reply.fail(e),
        });
    }

    /// A tab whose only pane is a mod (a file, a board).
    fn new_mod_tab(&mut self, space: Option<String>, name: &str, m: String, props: Value) -> Result<Value, String> {
        let space = space.or_else(|| self.model.active_space().map(|p| p.id.clone())).ok_or("no space yet: make one first (space.new)")?;
        let machine = self.model.space(&space).ok_or_else(|| format!("no space {space:?}"))?.machine.clone();
        self.require_document(machine.as_deref())?;
        let (tab, pane) = (self.model.fresh_id_on("tb", machine.as_deref()), self.model.fresh_id_on("p", machine.as_deref()));
        self.model.add_tab(&space, Tab::with_pane(&tab, name, Node::block(pane.clone(), Content::Mod { name: m, props })));
        self.model.active = Some(space.clone());
        Ok(json!({ "space": space, "tab": tab, "pane": pane }))
    }

    /// A new terminal beside a pane (default: the focused one), on the
    /// pane's machine, in its folder.
    fn split(&mut self, pane: Option<String>, axis: Axis, argv: Option<Vec<String>>, folder: Option<String>, reply: Reply) {
        let Some(target) = pane.or_else(|| self.model.focused_pane()) else { return reply.fail("no pane to split") };
        let (Some(p), Some(c)) = (self.model.space_of(&target), self.model.tab_of(&target)) else {
            return reply.fail(format!("no pane {target:?}"));
        };
        let (space, tab, fallback, machine) = (p.id.clone(), c.id.clone(), p.folder.clone(), p.machine.clone());
        let target_addr = self.model.address_of(&target);
        let follow = folder.is_none();
        let cwd = folder.unwrap_or_else(|| target_addr.as_ref().and_then(|a| self.cwds.get(a).cloned()).unwrap_or(fallback));
        // A split stays on the machine of the pane it splits (else its space's).
        let on = target_addr.as_deref().and_then(machine_of_address).map(str::to_owned).or(machine);
        let new = self.model.fresh_id_on("p", on.as_deref());
        self.spawn(Spawn { argv, cwd, follow, space, tab, on }, move |s, result| match result {
            Ok((addr, _)) if s.model.split(&target, axis, &new, &addr) => reply.value(json!({ "pane": new, "terminal": addr })),
            Ok((addr, _)) => {
                s.kill(&[addr]);
                reply.fail(format!("no pane {target:?}"));
            }
            Err(e) => reply.fail(e),
        });
    }

    /// A mod as a pane beside the target (a viewer next to the shell).
    fn split_mod(&mut self, pane: Option<String>, axis: Axis, name: String, props: Value) -> Result<Value, String> {
        let target = pane.or_else(|| self.model.focused_pane()).ok_or("no pane to split")?;
        let machine = self.model.space_of(&target).ok_or_else(|| format!("no pane {target:?}"))?.machine.clone();
        self.require_document(machine.as_deref())?;
        let new = self.model.fresh_id_on("p", machine.as_deref());
        self.model.split_with(&target, axis, Node::block(new.clone(), Content::Mod { name, props }));
        Ok(json!({ "pane": new }))
    }

    /// Gives a running terminal without a pane (a sub-agent) one in the
    /// shown space; already in a pane: focuses that.
    fn keep_terminal(&mut self, addr: &str, place: &str) -> Result<Value, String> {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
            return Ok(json!({ "pane": p }));
        }
        if !self.live.contains_key(addr) {
            return Err(format!("no running terminal {addr:?}"));
        }
        if self.changing.contains(addr) {
            return Err("This terminal already has a pane change in progress.".into());
        }
        let space = self.model.active_space().map(|p| p.id.clone()).ok_or("no space to keep it in")?;
        let machine = self.model.space(&space).and_then(|p| p.machine.clone());
        if machine_of_address(addr) != machine.as_deref() {
            return Err("A terminal's pane must stay on its own machine.".into());
        }
        self.require_document(machine.as_deref())?;
        let target = matches!(place, "right" | "down").then(|| self.model.focused_pane()).flatten();
        let tab = target
            .as_deref()
            .and_then(|p| self.model.tab_of(p))
            .map(|t| t.id.clone())
            .unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let name = self.live[addr]["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| self.model.agents.get(addr).and_then(|a| a.app.as_deref()).map(herdc_model::programs::name))
            .unwrap_or_else(|| addr.to_owned());
        let reply = json!({ "pending": true, "terminal": addr, "pane": pane, "tab": tab });
        let axis = if place == "right" { Axis::Row } else { Axis::Column };
        self.change_terminal(addr.to_owned(), TerminalChange::Keep { space, tab, pane, name, target, axis })?;
        Ok(reply)
    }
}

struct NewSpace {
    name: String,
    folder: Option<String>,
    tab: String,
    argv: Option<Vec<String>>,
    machine: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_arguments_are_rejected_for_every_command() {
        for c in COMMANDS {
            let error = validate_args(c, &json!({ "typo": "target" })).unwrap_err();
            assert!(error.contains(c.name) && error.contains("unknown argument(s): typo"), "{error}");
            assert!(validate_args(c, &Value::Null).is_ok());
            assert!(validate_args(c, &json!(["target"])).is_err());
            let args: serde_json::Map<String, Value> = c.args.iter().map(|a| (a.name.into(), json!("target"))).collect();
            assert!(validate_args(c, &Value::Object(args)).is_ok());
        }
    }

    #[test]
    fn names_are_unique_and_shorthand_fills_the_first_argument() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
        assert_eq!(shorthand(find("tab.select").unwrap(), "3"), json!({ "index": 3 }));
        assert_eq!(shorthand(find("pane.split").unwrap(), "right"), json!({ "direction": "right" }));
        assert_eq!(missing(find("space.move").unwrap(), &json!({ "id": " ", "index": 1 })), Some("id"));
        assert_eq!(missing(find("space.new").unwrap(), &json!({})), None);
    }

    #[test]
    fn agents_start_in_the_machines_own_shell_with_their_task() {
        let argv = agent_argv(Some(vec!["codex".into()]), Some(" fix it ")).unwrap();
        assert_eq!(argv[..2], ["sh", "-c"]);
        assert_eq!(argv[4..], ["codex", "fix it"]);
        assert!(agent_argv(Some(vec![]), Some("x")).is_none());
    }
}
