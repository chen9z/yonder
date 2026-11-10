//! The changes mod: what isn't committed in a repo, each file's diff, and
//! a commit of all of it (herdr-app `defaults/mods/changes/main.js`).

use super::{cut, said, There};
use std::collections::HashMap;

/// At most this many files, and lines of one diff.
pub const FILES: usize = 1000;
pub const LINES: usize = 1500;
const LINE: usize = 4000;

/// git reading without taking the index lock or refreshing it.
const READ: [&str; 5] = ["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "diff.autoRefreshIndex=false"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// `U`nmerged, `D`eleted, `A`dded or `M`odified.
    pub status: char,
    /// Not tracked yet.
    pub fresh: bool,
    pub add: u64,
    pub del: u64,
    pub binary: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub root: String,
    pub files: Vec<Change>,
    /// More than [`FILES`] changed.
    pub limited: bool,
    /// The repo has a commit.
    pub head: bool,
}

impl Changes {
    pub fn added(&self) -> u64 {
        self.files.iter().map(|f| f.add).sum()
    }

    pub fn deleted(&self) -> u64 {
        self.files.iter().map(|f| f.del).sum()
    }
}

/// The changes in the repo `cwd` is in.
pub async fn scan(there: &There, cwd: &str) -> Result<Changes, String> {
    let top = there.git(&["rev-parse", "--show-toplevel"], cwd).await?;
    if !top.ok() {
        return Err("Not a git repository".into());
    }
    let root = top.out().trim().to_owned();
    let status = read(there, &root, &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"]).await?;
    if !status.ok() {
        return Err(said(&status, "git status failed"));
    }
    let head = read(there, &root, &["rev-parse", "--verify", "HEAD"]).await?.ok();
    let numstats: &[&[&str]] = if head {
        &[&["diff", "HEAD", "--numstat", "-z", "--no-renames"]]
    } else {
        &[&["diff", "--cached", "--numstat", "-z", "--no-renames"], &["diff", "--numstat", "-z", "--no-renames"]]
    };
    let mut stats: HashMap<String, (u64, u64, bool)> = HashMap::new();
    for args in numstats {
        let ran = read(there, &root, args).await?;
        if !ran.ok() {
            return Err(said(&ran, "git diff failed"));
        }
        for record in ran.out().split('\0') {
            let mut parts = record.split('\t');
            let (Some(a), Some(d)) = (parts.next(), parts.next()) else { continue };
            let path = parts.collect::<Vec<_>>().join("\t");
            if path.is_empty() {
                continue;
            }
            let old = stats.entry(path).or_default();
            *old = (old.0 + a.parse().unwrap_or(0), old.1 + d.parse().unwrap_or(0), a == "-");
        }
    }
    let out = status.out();
    let entries: Vec<&str> = out.split('\0').filter(|e| !e.is_empty()).collect();
    let files = entries
        .iter()
        .take(FILES)
        .filter(|e| e.len() > 3)
        .map(|entry| {
            let (xy, path) = (&entry[..2], &entry[3..]);
            let fresh = xy == "??";
            let status = if xy.contains('U') || xy == "AA" || xy == "DD" {
                'U'
            } else if xy.contains('D') {
                'D'
            } else if fresh || xy.contains('A') {
                'A'
            } else {
                'M'
            };
            let (add, del, binary) = stats.get(path).copied().unwrap_or_default();
            Change { path: path.to_owned(), status, fresh, add, del, binary }
        })
        .collect();
    Ok(Changes { root, files, limited: entries.len() > FILES, head })
}

/// `git <READ> <args>` in the repo.
async fn read(there: &There, root: &str, args: &[&str]) -> Result<crate::exec::Ran, String> {
    there.git(&[&READ[..], args].concat(), root).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hunk,
    Add,
    Del,
    Same,
    /// Anything else: git's notes, or what the diff can't show.
    Other,
}

/// One row of a diff, numbered as its side counts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub number: Option<u64>,
    pub text: String,
}

/// A file's diff from HEAD (a new file: all of it added).
pub async fn diff(there: &There, changes: &Changes, file: &Change) -> Vec<Line> {
    let lines = match raw(there, changes, file).await {
        Ok(lines) => lines,
        Err(_) => vec!["Cannot preview this diff (too large or unreadable)".to_owned()],
    };
    let lines = lines.into_iter().take(LINES).map(|l| if l.chars().count() > LINE { format!("{}… (line truncated)", cut(&l, LINE)) } else { l });
    rows(lines)
}

async fn raw(there: &There, changes: &Changes, file: &Change) -> Result<Vec<String>, String> {
    let root = &changes.root;
    if file.fresh || (!changes.head && file.status == 'A') {
        let text = there.read_text(&format!("{root}/{}", file.path)).await.ok().flatten();
        return Ok(match text {
            None => vec!["Cannot preview this file (binary, too large, or unreadable)".into()],
            Some(t) if t.contains('\0') => vec!["Binary file · no text diff".into()],
            Some(t) => {
                let count = t.split('\n').count() - usize::from(t.ends_with('\n'));
                let shown = cut(&t, 400_000);
                let shown = shown.strip_suffix('\n').unwrap_or(shown);
                let mut lines = vec![format!("@@ -0,0 +1,{count} @@")];
                lines.extend(shown.split('\n').take(LINES).map(|l| format!("+{l}")));
                lines
            }
        });
    }
    let mut args = READ.to_vec();
    args.push("diff");
    if changes.head {
        args.push("HEAD");
    }
    args.extend(["--no-ext-diff", "--no-textconv", "--no-color", "--", &file.path]);
    let ran = there.git(&args, root).await?;
    if !ran.ok() {
        return Err(said(&ran, "git diff failed"));
    }
    let out = ran.out();
    let all: Vec<&str> = out.strip_suffix('\n').unwrap_or(&out).split('\n').collect();
    Ok(match all.iter().position(|l| l.starts_with("@@")) {
        Some(first) => all[first..].iter().map(|l| (*l).to_owned()).collect(),
        None => vec![if file.binary { "Binary file changed" } else { "No text diff available" }.to_owned()],
    })
}

/// Numbers each line by the side it is on (the mod's `diffRows`).
fn rows(lines: impl Iterator<Item = String>) -> Vec<Line> {
    let (mut old, mut new) = (0, 0);
    lines
        .map(|line| {
            if let Some((a, b)) = hunk(&line) {
                (old, new) = (a, b);
                return Line { kind: Kind::Hunk, number: None, text: line };
            }
            let rest = || line.get(1..).filter(|r| !r.is_empty()).unwrap_or(" ").to_owned();
            let (kind, number) = match line.chars().next() {
                Some('-') => (Kind::Del, Some(old)),
                Some('+') => (Kind::Add, Some(new)),
                Some(' ') => (Kind::Same, Some(new)),
                _ => return Line { kind: Kind::Other, number: None, text: line },
            };
            match kind {
                Kind::Del => old += 1,
                Kind::Add => new += 1,
                _ => (old, new) = (old + 1, new + 1),
            }
            Line { kind, number, text: rest() }
        })
        .collect()
}

/// `@@ -a[,n] +b`: where the hunk starts on each side.
fn hunk(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let new = rest.strip_prefix('+')?;
    let number = |s: &str| s.split(',').next()?.parse().ok();
    let new_end = new.find(|c: char| !c.is_ascii_digit()).unwrap_or(new.len());
    Some((number(old)?, number(&new[..new_end])?))
}

/// Commits everything: `git add -A`, then `git commit -m`.
pub async fn commit(there: &There, root: &str, message: &str) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Type a commit message.".into());
    }
    for args in [&["add", "-A"][..], &["commit", "-m", message]] {
        let ran = there.git(args, root).await?;
        if !ran.ok() {
            return Err(format!("Commit failed: {}", said(&ran, "git failed")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_numbered_by_their_side() {
        let diff = ["@@ -3,2 +3,3 @@ fn", " same", "-gone", "+came", "+also", "\\ No newline at end of file"];
        let rows = rows(diff.iter().map(|l| (*l).to_owned()));
        let numbered: Vec<_> = rows.iter().map(|r| (r.kind, r.number, r.text.as_str())).collect();
        assert_eq!(
            numbered,
            [
                (Kind::Hunk, None, "@@ -3,2 +3,3 @@ fn"),
                (Kind::Same, Some(3), "same"),
                (Kind::Del, Some(4), "gone"),
                (Kind::Add, Some(4), "came"),
                (Kind::Add, Some(5), "also"),
                (Kind::Other, None, "\\ No newline at end of file"),
            ]
        );
    }

    #[test]
    fn a_blank_changed_line_keeps_a_space() {
        let rows = rows(["@@ -0,0 +1 @@", "+"].iter().map(|l| (*l).to_owned()));
        assert_eq!(rows[1].text, " ");
        assert_eq!(hunk("@@ -0,0 +1 @@"), Some((0, 1)));
    }
}
