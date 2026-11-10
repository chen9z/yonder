//! The worktree mod: a new checkout on a branch, the checkouts there are,
//! and removing one with what it would lose said first (herdr-app
//! `defaults/mods/worktree/main.js`). Opening the space is the caller's:
//! `space.new {folder, machine}`, unnamed so it follows the checkout.

use super::{said, There, LONG};
use crate::exec::Ran;
use crate::view::SpaceView;

/// A repo a folder is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// The checkout the folder is in.
    pub top: String,
    /// The main checkout.
    pub main: String,
    pub name: String,
    /// `top` is a linked worktree, not the main checkout.
    pub linked: bool,
}

async fn git(there: &There, cwd: &str, args: &[&str]) -> Result<Ran, String> {
    there.git(args, cwd).await
}

fn out(ran: &Ran) -> String {
    ran.out().trim().to_owned()
}

/// None outside git.
pub async fn repo_of(there: &There, folder: &str) -> Result<Option<Repo>, String> {
    let top = git(there, folder, &["rev-parse", "--show-toplevel"]).await?;
    let common = git(there, folder, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await?;
    if !top.ok() || !common.ok() {
        return Ok(None);
    }
    let (top, common) = (out(&top), out(&common));
    let main = common.strip_suffix("/.git").unwrap_or(&common).to_owned();
    let name = main.rsplit('/').next().unwrap_or_default().to_owned();
    Ok(Some(Repo { linked: main != top, top, main, name }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub path: String,
    /// None: detached.
    pub branch: Option<String>,
    pub main: bool,
    /// When its branch was last committed to (unix seconds).
    pub at: i64,
}

/// Every checkout of `repo`: the main one first, then the most recently
/// committed.
pub async fn checkouts(there: &There, repo: &Repo) -> Result<Vec<Checkout>, String> {
    let list = git(there, &repo.main, &["worktree", "list", "--porcelain"]).await?;
    if !list.ok() {
        return Ok(Vec::new());
    }
    let refs = git(there, &repo.main, &["for-each-ref", "--format=%(refname:short)%09%(committerdate:unix)", "refs/heads"]).await?;
    let refs = out(&refs);
    let when = |branch: &str| {
        refs.lines().filter_map(|l| l.split_once('\t')).find(|(b, _)| *b == branch).and_then(|(_, at)| at.parse().ok()).unwrap_or(0)
    };
    let list = out(&list);
    let mut all: Vec<Checkout> = Vec::new();
    for block in list.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let lines: Vec<&str> = block.trim_matches('\n').lines().collect();
        let Some(path) = lines.iter().find_map(|l| l.strip_prefix("worktree ")) else { continue };
        if lines.iter().any(|l| *l == "bare" || l.starts_with("prunable")) {
            continue;
        }
        let branch = lines.iter().find_map(|l| l.strip_prefix("branch ")).map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
        let at = branch.as_deref().map_or(0, when);
        all.push(Checkout { path: path.to_owned(), branch, main: all.is_empty(), at });
    }
    all.sort_by_key(|c| (!c.main, std::cmp::Reverse(c.at)));
    Ok(all)
}

/// A branch as a folder name.
pub fn slug(branch: &str) -> String {
    let mut out = String::new();
    for c in branch.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Where a new checkout of `repo` on `branch` goes.
pub fn path(home: &str, repo: &Repo, branch: &str) -> String {
    format!("{home}/.herdr/worktrees/{}/{}", repo.name, slug(branch))
}

/// Checks out `branch` (made from here when it doesn't exist) in a new
/// worktree; returns its folder.
pub async fn create(there: &There, repo: &Repo, home: &str, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Type a branch name.".into());
    }
    let path = path(home, repo, branch);
    let head = format!("refs/heads/{branch}");
    let exists = git(there, &repo.main, &["show-ref", "--verify", "--quiet", &head]).await?.ok();
    let add: &[&str] = if exists { &["worktree", "add", &path, branch] } else { &["worktree", "add", "-b", branch, &path, "HEAD"] };
    let ran = there.run(&[&["git"], add].concat(), Some(&repo.top), LONG).await?;
    if !ran.ok() {
        return Err(said(&ran, "git worktree add failed."));
    }
    Ok(path)
}

/// The space showing a checkout: its folder is in it, and in no deeper
/// checkout.
pub fn space_of<'a>(path: &str, all: &[Checkout], on: Option<&str>, spaces: &'a [SpaceView]) -> Option<&'a SpaceView> {
    let inside = |p: &str, dir: &str| p == dir || p.strip_prefix(dir).is_some_and(|r| r.starts_with('/'));
    spaces.iter().find(|s| {
        let at = s.place().replace('\\', "/");
        s.machine.as_deref() == on
            && inside(&at, path)
            && !all.iter().any(|c| c.path != path && c.path.len() > path.len() && inside(&at, &c.path))
    })
}

/// What removing a checkout would lose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Risk {
    /// Uncommitted changes.
    pub dirty: bool,
    /// Submodules with commits nowhere else.
    pub unpushed: Vec<String>,
    /// It has submodules at all (git removes those only when forced).
    pub modules: bool,
}

pub async fn risk(there: &There, top: &str) -> Result<Risk, String> {
    let status = git(there, top, &["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"]).await?;
    if !status.ok() {
        return Err(said(&status, "git status failed."));
    }
    let (modules, unpushed) = submodules(there, top).await?;
    Ok(Risk { dirty: !out(&status).is_empty(), unpushed, modules: modules > 0 })
}

impl Risk {
    /// Takes in a later look; true when it found more at risk than this
    /// showed (the person is told once more before it goes).
    pub fn grew(&mut self, now: Risk) -> bool {
        let grew = (now.dirty && !self.dirty) || now.unpushed.iter().any(|n| !self.unpushed.contains(n));
        self.dirty |= now.dirty;
        self.modules |= now.modules;
        for name in now.unpushed {
            if !self.unpushed.contains(&name) {
                self.unpushed.push(name);
            }
        }
        grew
    }
}

/// Deletes the checkout (its space closed first, by the caller: a shell in
/// the folder keeps it busy on Windows). The branch stays.
pub async fn remove(there: &There, repo: &Repo, risk: &Risk) -> Result<(), String> {
    let mut args = vec!["git", "worktree", "remove"];
    if risk.dirty || risk.modules {
        args.push("--force");
    }
    args.push(&repo.top);
    let ran = there.run(&args, Some(&repo.main), LONG).await?;
    if ran.ok() { Ok(()) } else { Err(said(&ran, "git worktree remove failed.")) }
}

/// A checkout's submodules (nested too), and those with commits that are
/// on no remote and aren't the commit the parent pins.
async fn submodules(there: &There, top: &str) -> Result<(usize, Vec<String>), String> {
    let dir = git(there, top, &["rev-parse", "--absolute-git-dir"]).await?;
    if !dir.ok() {
        return Err(said(&dir, "git rev-parse failed."));
    }
    let dir = out(&dir);
    let file = format!("{top}/.gitmodules");
    let mut found = Vec::new();
    modules(there, &dir, vec!["--file".into(), file], String::new(), &mut found).await?;
    let mut unpushed = Vec::new();
    for (name, module, pinned) in &found {
        let mut args = vec!["--git-dir", module, "rev-list", "-n1", "--all", "--not", "--remotes"];
        if !pinned.is_empty() {
            args.push(pinned);
        }
        let local = git(there, &dir, &args).await?;
        if !local.ok() {
            return Err(said(&local, &format!("Couldn't check submodule {name}.")));
        }
        if !out(&local).is_empty() {
            unpushed.push(name.clone());
        }
    }
    Ok((found.len(), unpushed))
}

/// The submodule repos git keeps under `git_dir`, checked out ones only:
/// (name, git dir, pinned commit).
async fn modules(there: &There, git_dir: &str, config: Vec<String>, parent: String, found: &mut Vec<(String, String, String)>) -> Result<(), String> {
    let mut args = vec!["--git-dir", git_dir, "config"];
    args.extend(config.iter().map(String::as_str));
    args.extend(["-z", "--get-regexp", "^submodule\\..*\\.path$"]);
    let keys = git(there, git_dir, &args).await?;
    if !keys.ok() {
        return Ok(());
    }
    let keys = keys.out();
    for record in keys.split('\0').filter(|r| !r.is_empty()) {
        let Some((key, path)) = record.split_once('\n') else { continue };
        let name = key.strip_prefix("submodule.").and_then(|k| k.strip_suffix(".path")).unwrap_or(key);
        let dir = format!("{git_dir}/modules/{name}");
        if !git(there, git_dir, &["--git-dir", &dir, "rev-parse", "--git-dir"]).await?.ok() {
            continue;
        }
        let pinned = git(there, git_dir, &["--git-dir", git_dir, "rev-parse", &format!("HEAD:{path}")]).await?;
        let pinned = if pinned.ok() { out(&pinned) } else { String::new() };
        let full = format!("{parent}{name}");
        found.push((full.clone(), dir.clone(), pinned));
        if git(there, git_dir, &["--git-dir", &dir, "cat-file", "-e", "HEAD:.gitmodules"]).await?.ok() {
            Box::pin(modules(there, &dir, vec!["--blob".into(), "HEAD:.gitmodules".into()], format!("{full}/"), found)).await?;
        }
    }
    Ok(())
}

/// How long ago, as the worktree list says it.
pub fn ago(seconds: i64) -> String {
    let d = seconds.max(0) as f64;
    match d {
        d if d < 3600.0 => format!("{}m", (d / 60.0).round().max(1.0)),
        d if d < 86400.0 => format!("{}h", (d / 3600.0).round()),
        d if d < 86400.0 * 60.0 => format!("{}d", (d / 86400.0).round()),
        d => format!("{}mo", (d / 86400.0 / 30.0).round()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_become_folder_names() {
        assert_eq!(slug("feat/phone ui"), "feat-phone-ui");
        assert_eq!(slug("--a//b--"), "a-b");
        assert_eq!(slug("v1.2_x"), "v1.2_x");
    }

    #[test]
    fn ages_read_as_the_list_shows_them() {
        assert_eq!(ago(5), "1m");
        assert_eq!(ago(7200), "2h");
        assert_eq!(ago(86400 * 3), "3d");
        assert_eq!(ago(86400 * 90), "3mo");
    }

    #[test]
    fn more_at_risk_is_said_once() {
        let mut seen = Risk { dirty: false, unpushed: vec!["a".into()], modules: true };
        assert!(!seen.grew(Risk { dirty: false, unpushed: vec!["a".into()], modules: true }));
        assert!(seen.grew(Risk { dirty: true, unpushed: vec![], modules: true }));
        assert!(!seen.grew(Risk { dirty: true, unpushed: vec![], modules: true }));
        assert!(seen.dirty);
    }
}
