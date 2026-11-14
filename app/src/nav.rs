//! The pages pushed over the home page (the desktop's sidebar), kept as
//! routes: the workspace first, the others over it.

use herdc_engine::tools::changes::Changes;
use herdc_engine::tools::Place;
use nsh_core::prelude::*;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// The shown space's tabs and panes.
    Workspace,
    Settings,
    EditConnection,
    Commands,
    /// A folder's tree.
    Files(Place),
    /// A file of it, by its path under the folder; at a line (from 1)
    /// when one was said with it.
    File { place: Place, path: String, line: Option<u32> },
    /// The changes of the repo a folder is in.
    Changes(Place),
    /// One changed file's diff.
    Diff { place: Place, changes: Arc<Changes>, file: usize },
    /// The checkouts of the repo a folder is in.
    Worktrees(Place),
    /// A terminal without a pane (a sub-agent), looked at.
    Peek(String),
    /// Another machine for the computer to reach.
    AddMachine,
    /// A terminal's text, to select from.
    Text(Arc<String>),
    /// One step the main agent took (a tool it used), whole.
    Step(Arc<herdc_engine::tools::transcript::Call>),
    /// The key bar's keys, to choose.
    KeyBar,
    /// Every terminal on every machine, found by typing.
    Search,
}

impl Route {
    pub fn key(&self) -> String {
        match self {
            Route::Diff { changes, file, .. } => format!("Diff {} {file}", changes.root),
            Route::Text(text) => format!("Text {:p}", Arc::as_ptr(text)),
            Route::Step(call) => format!("Step {:p}", Arc::as_ptr(call)),
            other => format!("{other:?}"),
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct Routes(pub Vec<Route>);

pub fn routes() -> Signal<Routes> {
    global::<Routes>()
}

/// Shows a page over the ones shown (the terminal's keyboard goes).
pub fn push(route: Route) {
    dismiss_keyboard();
    routes().update(|r| r.0.push(route));
}

/// Back to the first `depth` pages.
pub fn truncate(depth: usize) {
    routes().update(|r| r.0.truncate(depth));
}

/// The page shown instead by another (the next file's diff).
pub fn replace(route: Route) {
    routes().update(|r| {
        r.0.pop();
        r.0.push(route);
    });
}

/// Back one page.
pub fn back() {
    routes().update(|r| { r.0.pop(); });
}

/// Back to the home page.
pub fn home() {
    truncate(0);
}

/// To the workspace, the pages over it gone.
pub fn workspace() {
    dismiss_keyboard();
    routes().set(Routes(vec![Route::Workspace]));
}
