//! The files mod: a folder's entries, the git marks of its files, and a
//! file's text to preview (herdr-app `defaults/mods/files/main.js`).

use super::{cut, There};
use std::collections::HashMap;

/// At most this many entries of one folder, and of the whole tree.
pub const PER_FOLDER: usize = 500;
pub const TOTAL: usize = 5000;

/// The longest text shown.
const PREVIEW: usize = 400_000;

/// Lists `$1` with hidden files, not `.git`, as `kind\0name\0` pairs; one
/// more than [`PER_FOLDER`] tells it is cut.
const LIST: &str = r#"n=0; for p in "$1"/* "$1"/.[!.]* "$1"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; name=${p##*/}; [ "$name" = .git ] && continue; if [ -d "$p" ]; then kind=d; else kind=f; fi; printf "%s\0%s\0" "$kind" "$name"; n=$((n+1)); [ "$n" -gt 500 ] && break; done; test -d "$1""#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// A folder's entries, folders first, and whether there were more.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub limited: bool,
}

pub async fn list(there: &There, path: &str) -> Result<Listing, String> {
    let mut entries = match there.run(&["sh", "-c", LIST, "files", path], None, super::TIMEOUT).await {
        Ok(ran) if ran.ok() => {
            let out = ran.out();
            let parts: Vec<&str> = out.split('\0').collect();
            parts.chunks_exact(2).map(|p| Entry { dir: p[0] == "d", name: p[1].to_owned() }).collect::<Vec<_>>()
        }
        Ok(ran) => return Err(super::said(&ran, "Cannot list this folder")),
        Err(e) if e.contains("cannot start sh: program not found") => windows(there, path).await?,
        Err(e) => return Err(e),
    };
    let limited = entries.len() > PER_FOLDER;
    entries.truncate(PER_FOLDER);
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    Ok(Listing { entries, limited })
}

/// The same from PowerShell, where there is no sh.
async fn windows(there: &There, path: &str) -> Result<Vec<Entry>, String> {
    let quoted: String = path.chars().flat_map(|c| if matches!(c, '\'' | '‘' | '’' | '‚' | '‛') { vec![c, c] } else { vec![c] }).collect();
    let script = format!(
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); & {{ param($p) @(Get-ChildItem -LiteralPath $p -Force -ErrorAction Stop | Where-Object Name -ne '.git' | Select-Object -First 501 | ForEach-Object {{ @{{ name=$_.Name; dir=($_.PSIsContainer -and (-not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not $_.LinkType -or ($_.Target -and [IO.Directory]::Exists(([IO.Path]::Combine([IO.Path]::GetDirectoryName($_.FullName), $_.Target[0])))))) }} }}) | ConvertTo-Json -Compress }} '{quoted}'"
    );
    let ran = there.run(&["powershell", "-NoProfile", "-Command", &script], None, super::TIMEOUT).await?;
    if !ran.ok() {
        return Err(super::said(&ran, "Cannot list this folder"));
    }
    let v: serde_json::Value = serde_json::from_str(ran.out().trim()).unwrap_or(serde_json::Value::Null);
    let all = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => Vec::new(),
        one => vec![one],
    };
    Ok(all.iter().map(|e| Entry { name: e["name"].as_str().unwrap_or_default().to_owned(), dir: e["dir"] == true }).collect())
}

/// The files git counts as changed in the repo `cwd` is in, by their full
/// path: `A`dded, `U`nmerged, `D`eleted or `M`odified. Empty out of a repo.
pub async fn marks(there: &There, cwd: &str) -> HashMap<String, char> {
    let mut marks = HashMap::new();
    let Ok(top) = there.git(&["rev-parse", "--show-toplevel"], cwd).await else { return marks };
    if !top.ok() {
        return marks;
    }
    let top = top.out().trim().to_owned();
    let args = ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"];
    let Ok(status) = there.git(&args, cwd).await else { return marks };
    for entry in status.out().split('\0').filter(|e| e.len() > 3).take(TOTAL) {
        let (xy, path) = (&entry[..2], &entry[3..]);
        let mark = if xy == "??" || xy.contains('A') {
            'A'
        } else if xy.contains('U') {
            'U'
        } else if xy.contains('D') {
            'D'
        } else {
            'M'
        };
        marks.insert(format!("{top}/{path}"), mark);
    }
    marks
}

/// A file as the preview shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    Text { text: String, cut: bool },
    /// Not there any more.
    Gone,
    Binary,
    /// Too large, or unreadable.
    Unreadable,
}

impl Preview {
    /// What to say instead of the text.
    pub fn said(&self) -> Option<&'static str> {
        match self {
            Preview::Text { .. } => None,
            Preview::Gone => Some("This file is no longer available"),
            Preview::Binary => Some("Binary file · no text preview"),
            Preview::Unreadable => Some("Cannot preview this file (binary, too large, or unreadable)"),
        }
    }
}

pub async fn preview(there: &There, path: &str) -> Preview {
    match there.read_text(path).await {
        Ok(None) => Preview::Gone,
        Ok(Some(text)) if text.contains('\0') => Preview::Binary,
        Ok(Some(text)) => {
            let shown = cut(&text, PREVIEW);
            if shown.len() < text.len() {
                Preview::Text { text: format!("{shown}\n… (file truncated)"), cut: true }
            } else {
                Preview::Text { text, cut: false }
            }
        }
        Err(_) => Preview::Unreadable,
    }
}
