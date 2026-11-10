//! The work of herdr-app's built-in mods, without their UI: the files
//! tree, the uncommitted changes and their commit, and worktrees. Every
//! line they run is the mods' own (herdr-app `defaults/mods/*/main.js`),
//! through `machine.exec` where the mods use `exec(.., {on})`.
//!
//! Adapted from herdr-app, Apache-2.0.

pub mod agents;
pub mod machines;
pub mod changes;
pub mod files;
pub mod worktree;

use crate::exec::{exec, Ran};
use crate::view::{SpaceView, View};
use herdc_wire::Client;
use std::time::Duration;

/// The mods' `exec` limit, and the one for `git worktree add`/`remove`.
pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const LONG: Duration = Duration::from_secs(600);

/// `readTextFile`'s limit (herdr-app `mod_files.rs`).
pub const TEXT_LIMIT: usize = 1024 * 1024;

/// A folder on a machine: the mods' `location()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    /// None: the core's own machine.
    pub on: Option<String>,
    pub folder: String,
}

impl View {
    /// Where the focused pane is: its folder, else the space's (the mods'
    /// `location()`).
    pub fn location(&self) -> Option<Place> {
        let space = self.active_space()?;
        let pane = space.tabs.iter().find(|t| t.active).and_then(|t| t.panes.iter().find(|p| p.focused));
        let folder = pane.and_then(|p| p.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| space.folder.clone());
        Some(Place { on: space.machine.clone(), folder })
    }
}

impl SpaceView {
    /// Where a space works: its first pane with a folder, else its own
    /// (the mods' `place()`).
    pub fn place(&self) -> String {
        let cwd = self.tabs.iter().flat_map(|t| &t.panes).find_map(|p| p.cwd.clone().filter(|c| !c.is_empty()));
        cwd.unwrap_or_else(|| self.folder.clone())
    }
}

/// A machine, through a core.
#[derive(Clone)]
pub struct There {
    pub core: Client,
    pub on: Option<String>,
}

impl There {
    pub fn new(core: Client, on: Option<String>) -> There {
        There { core, on }
    }

    pub async fn run(&self, argv: &[&str], cwd: Option<&str>, timeout: Duration) -> Result<Ran, String> {
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
        exec(&self.core, self.on.as_deref(), &argv, cwd, timeout).await
    }

    /// `git <args>` in `cwd`, in the mods' default time.
    pub async fn git(&self, args: &[&str], cwd: &str) -> Result<Ran, String> {
        self.run(&[&["git"], args].concat(), Some(cwd), TIMEOUT).await
    }

    /// A text file there; None when it isn't (or can't be read: the mods'
    /// `readTextFile(.., {on})` doesn't tell the two apart).
    pub async fn read_text(&self, path: &str) -> Result<Option<String>, String> {
        let ran = match self.run(&["cat", "--", path], None, TIMEOUT).await {
            Err(e) if e.contains("cannot start") => {
                let windows = path.replace('/', "\\");
                self.run(&["cmd", "/d", "/c", "type", &windows], None, TIMEOUT).await?
            }
            other => other?,
        };
        if !ran.ok() {
            return Ok(None);
        }
        if ran.stdout.len() > TEXT_LIMIT {
            return Err("Text file exceeds 1 MiB".into());
        }
        Ok(Some(ran.out()))
    }

    /// The machine's home folder, with `/` between parts.
    pub async fn home(&self) -> Result<String, String> {
        let posix = self.run(&["sh", "-c", "printf %s \"$HOME\""], None, TIMEOUT).await.ok().filter(Ran::ok);
        let home = match posix {
            Some(r) if !r.stdout.is_empty() => r.out(),
            _ => {
                let ps = ["powershell.exe", "-NoProfile", "-Command", "[Console]::Write($HOME)"];
                self.run(&ps, None, TIMEOUT).await.ok().filter(Ran::ok).map(|r| r.out()).unwrap_or_default()
            }
        };
        let home = home.trim().replace('\\', "/");
        if home.is_empty() { Err("Could not find the remote home folder.".into()) } else { Ok(home) }
    }
}

/// What a failed run said: stderr, else stdout, else `fallback`.
fn said(ran: &Ran, fallback: &str) -> String {
    let err = String::from_utf8_lossy(&ran.stderr).trim().to_owned();
    if !err.is_empty() {
        return err;
    }
    let out = ran.out().trim().to_owned();
    if out.is_empty() { fallback.to_owned() } else { out }
}

/// `text` cut to `max` chars.
fn cut(text: &str, max: usize) -> &str {
    text.char_indices().nth(max).map_or(text, |(i, _)| &text[..i])
}
