//! Bot definitions: `<home>/bots/<name>/bot.toml`, with the optional
//! `template.md`, `AGENTS.md` and `MEMORY.md` beside it. Reading one never
//! fails: what is wrong with it is a list of problems, so one broken
//! definition never hides the others. Yonder only writes a new bot's
//! files (`create`); it never changes them after.

use std::path::{Path, PathBuf};

use yonder_model::{BotDef, BotKind};

/// The files a bot's directory may hold, in the order they are shown.
pub const FILES: &[&str] = &["bot.toml", "template.md", "AGENTS.md", "MEMORY.md"];

/// The keys `bot.toml` knows; others are warned about and left alone.
const KNOWN: &[&str] = &["name", "description", "kind", "machine", "cwd", "harness", "model", "args", "memory_id", "env"];

/// Variables a bot's `env` may not name: the system's, and Yonder's own.
const RESERVED_ENV: &[&str] = &["PATH", "HOME", "SHELL", "USER", "LOGNAME", "PWD", "TMPDIR", "TERM", "LANG", "TZ"];
const RESERVED_ENV_PREFIXES: &[&str] = &["YONDER_", "LC_", "DYLD_", "LD_"];

/// What `bot new` writes in `description`, to be said over.
const PLACEHOLDER: &str = "TODO: ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub level: Level,
    pub message: String,
}

/// A bot's directory as read: its definition if it holds one, what is
/// wrong with it, and which of `FILES` are there.
#[derive(Clone, Debug)]
pub struct Bot {
    /// Its directory's name.
    pub name: String,
    pub dir: PathBuf,
    /// None when an error keeps it from being one.
    pub def: Option<BotDef>,
    pub problems: Vec<Problem>,
    pub files: Vec<(&'static str, PathBuf)>,
}

impl Bot {
    /// An error keeps it from being used.
    pub fn invalid(&self) -> bool {
        self.def.is_none()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("{0:?} is not a bot name")]
    BadName(String),
    #[error("bot {0} exists, at {1}")]
    Exists(String, PathBuf),
    #[error("no bot {0}")]
    NotFound(String),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

impl BotError {
    /// A word that names the error for scripts; stays the same across versions.
    pub fn code(&self) -> &'static str {
        match self {
            BotError::BadName(_) => "bad_name",
            BotError::Exists(..) => "bot_exists",
            BotError::NotFound(_) => "not_found",
            BotError::Io(..) => "home",
        }
    }
}

/// `^[a-z][a-z0-9-]{0,31}$`.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `^[A-Z_][A-Z0-9_]*$`, and none of the reserved names.
pub fn env_problem(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let well_formed = chars.next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !well_formed {
        Some(format!("env {name:?} is not a variable name: use A-Z, 0-9 and _, not starting with a digit"))
    } else if RESERVED_ENV.contains(&name) || RESERVED_ENV_PREFIXES.iter().any(|p| name.starts_with(p)) {
        Some(format!("env {name} is the system's or Yonder's own; a bot cannot be given it"))
    } else {
        None
    }
}

/// The bot in `dir`.
pub fn load(dir: &Path) -> Bot {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let files = FILES.iter().map(|f| (*f, dir.join(f))).filter(|(_, p)| p.is_file()).collect();
    let mut bot = Bot { name, dir: dir.into(), def: None, problems: Vec::new(), files };
    let path = dir.join("bot.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (def, problems) = parse(&bot.name, &text);
            bot.def = def;
            bot.problems = problems;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bot.problems.push(error("no bot.toml".into())),
        Err(e) => bot.problems.push(error(format!("reading bot.toml: {e}"))),
    }
    bot
}

/// Every bot in `bots` (`home::bots_dir`), by name; none if it isn't there.
/// Entries that aren't directories, or start with `.`, are not bots.
pub fn list(bots: &Path) -> Result<Vec<Bot>, BotError> {
    let entries = match std::fs::read_dir(bots) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(BotError::Io(bots.into(), e)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| BotError::Io(bots.into(), e))?;
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if !hidden && entry.path().is_dir() {
            found.push(load(&entry.path()));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// The bot `name` in `bots`.
pub fn find(bots: &Path, name: &str) -> Result<Bot, BotError> {
    let dir = bots.join(name);
    if !valid_name(name) {
        return Err(BotError::BadName(name.into()));
    }
    if !dir.is_dir() {
        return Err(BotError::NotFound(name.into()));
    }
    Ok(load(&dir))
}

/// A new bot `name` in `bots`: its directory, a commented `bot.toml`, and
/// empty `template.md` and `AGENTS.md`. Never over one that exists.
pub fn create(bots: &Path, name: &str, kind: BotKind, harness: Option<&str>) -> Result<PathBuf, BotError> {
    if !valid_name(name) {
        return Err(BotError::BadName(name.into()));
    }
    let dir = bots.join(name);
    let io = |p: &Path, e| BotError::Io(p.into(), e);
    std::fs::create_dir_all(bots).map_err(|e| io(bots, e))?;
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(BotError::Exists(name.into(), dir)),
        Err(e) => return Err(io(&dir, e)),
    }
    for (file, text) in [("bot.toml", template(name, kind, harness)), ("template.md", String::new()), ("AGENTS.md", String::new())] {
        let path = dir.join(file);
        std::fs::write(&path, text).map_err(|e| io(&path, e))?;
    }
    Ok(dir)
}

/// The `bot.toml` that `create` writes. Its keys are at the top level, so
/// a line added at the end (such as `cwd = ...`) is one of them.
fn template(name: &str, kind: BotKind, harness: Option<&str>) -> String {
    let harness = toml_string(harness.unwrap_or(""));
    let cwd = match kind {
        BotKind::Persistent => "required for a persistent bot",
        BotKind::Oneoff => "a oneoff bot may leave it out: its task says where",
    };
    format!(
        "# The bot {name}. Fields: docs/spec.md, \"bot\". Check it with: yonder bot check {name}\n\
         name = \"{name}\"\n\
         description = \"{PLACEHOLDER}what {name} does, in one line\"\n\
         # persistent | oneoff\n\
         kind = \"{kind}\"\n\
         # local, or an hc machine name\n\
         machine = \"local\"\n\
         # codex | pi | claude | another program\n\
         harness = {harness}\n\
         # Where it works; {cwd}.\n\
         # cwd = \"~/bots/{name}\"\n\
         # model = \"\"\n\
         # args = []\n\
         # Its Nowledge identity (NMEM_AGENT_ID).\n\
         # memory_id = \"{name}\"\n\
         # Names only; the values are kept in the keychain.\n\
         # env = [\"EXAMPLE_API_KEY\"]\n"
    )
}

/// `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn error(message: String) -> Problem {
    Problem { level: Level::Error, message }
}

fn warning(message: String) -> Problem {
    Problem { level: Level::Warning, message }
}

/// The definition in `text`, the `bot.toml` of the directory `dir_name`,
/// if no error keeps it from being one; and every problem with it.
pub fn parse(dir_name: &str, text: &str) -> (Option<BotDef>, Vec<Problem>) {
    let mut problems = Vec::new();
    let table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(e) => {
            let line = e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
            let at = line.map_or(String::new(), |l| format!(" line {l}"));
            let said = e.message().lines().next().unwrap_or("not TOML").trim();
            return (None, vec![error(format!("bot.toml{at}: {said}"))]);
        }
    };
    let mut string = |key: &str, required: bool| -> Option<String> {
        match table.get(key) {
            None if required => {
                problems.push(error(format!("{key} is required")));
                None
            }
            None => None,
            Some(toml::Value::String(s)) if required && s.trim().is_empty() => {
                problems.push(error(format!("{key} is required, and empty")));
                None
            }
            Some(toml::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                problems.push(error(format!("{key} must be a string")));
                None
            }
        }
    };
    let name = string("name", true);
    let description = string("description", true);
    let kind_word = string("kind", true);
    let machine = string("machine", false);
    let cwd = string("cwd", false).filter(|c| !c.trim().is_empty());
    let harness = string("harness", true);
    let model = string("model", false).filter(|m| !m.is_empty());
    let memory_id = string("memory_id", false).filter(|m| !m.is_empty());
    let mut strings = |key: &str| -> Vec<String> {
        match table.get(key) {
            None => Vec::new(),
            Some(toml::Value::Array(items)) if items.iter().all(toml::Value::is_str) => {
                items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
            }
            Some(_) => {
                problems.push(error(format!("{key} must be a list of strings")));
                Vec::new()
            }
        }
    };
    let args = strings("args");
    let env = strings("env");

    if !valid_name(dir_name) {
        problems.push(error(format!("the directory {dir_name:?} is not a bot name: ^[a-z][a-z0-9-]{{0,31}}$")));
    }
    if let Some(name) = &name {
        if !valid_name(name) {
            problems.push(error(format!("name {name:?} is not a bot name: ^[a-z][a-z0-9-]{{0,31}}$")));
        } else if name != dir_name {
            problems.push(error(format!("name {name:?} is not its directory's name {dir_name:?}")));
        }
    }
    if description.as_deref().is_some_and(|d| d.starts_with(PLACEHOLDER)) {
        problems.push(warning("description is still the placeholder bot new wrote".into()));
    }
    let kind = kind_word.as_deref().and_then(|k| {
        let kind = BotKind::parse(k);
        if kind.is_none() {
            problems.push(error(format!("kind {k:?} is not persistent or oneoff")));
        }
        kind
    });
    if kind == Some(BotKind::Persistent) && cwd.is_none() {
        problems.push(error("a persistent bot needs cwd, where it works".into()));
    }
    for (i, var) in env.iter().enumerate() {
        if let Some(p) = env_problem(var) {
            problems.push(error(p));
        } else if env[..i].contains(var) {
            problems.push(warning(format!("env {var} is named twice")));
        }
    }
    for key in table.keys().filter(|k| !KNOWN.contains(&k.as_str())) {
        problems.push(warning(format!("unknown field {key}, left alone")));
    }

    let def = match (name, description, kind, harness) {
        (Some(name), Some(description), Some(kind), Some(harness)) if !problems.iter().any(|p| p.level == Level::Error) => Some(BotDef {
            name,
            description,
            kind,
            machine: machine.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "local".into()),
            cwd,
            harness,
            model,
            args,
            memory_id,
            env,
        }),
        _ => None,
    };
    (def, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(problems: &[Problem]) -> Vec<&str> {
        problems.iter().filter(|p| p.level == Level::Error).map(|p| p.message.as_str()).collect()
    }

    const TRAINER: &str = "name = \"trainer\"\ndescription = \"GPU training\"\nkind = \"persistent\"\ncwd = \"~/bots/trainer\"\nharness = \"codex\"\n";

    #[test]
    fn names() {
        for good in ["trainer", "ops", "a", "nova-2", "a234567890123456789012345678901b"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "Trainer", "2ops", "-ops", "ops_1", "a2345678901234567890123456789012c", "nova.x", "../ops"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn env_names() {
        assert_eq!(env_problem("EXAMPLE_API_KEY"), None);
        assert_eq!(env_problem("_X1"), None);
        for bad in ["example", "1X", "A-B", "", "PATH", "HOME", "SHELL", "YONDER_HOME", "LC_ALL", "DYLD_INSERT_LIBRARIES"] {
            assert!(env_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn a_whole_definition() {
        let text = format!("{TRAINER}model = \"m1\"\nargs = [\"--no-daemon\"]\nmemory_id = \"trainer\"\nenv = [\"EXAMPLE_API_KEY\"]\nmachine = \"gpu-box\"\n");
        let (def, problems) = parse("trainer", &text);
        assert_eq!(problems, []);
        let def = def.unwrap();
        assert_eq!((def.kind, def.machine.as_str(), def.cwd.as_deref()), (BotKind::Persistent, "gpu-box", Some("~/bots/trainer")));
        assert_eq!((def.args, def.env), (vec!["--no-daemon".to_owned()], vec!["EXAMPLE_API_KEY".to_owned()]));
        // Left out: local.
        assert_eq!(parse("trainer", TRAINER).0.unwrap().machine, "local");
    }

    #[test]
    fn what_is_wrong_is_said_one_by_one() {
        let (def, problems) = parse("trainer", "kind = \"daemon\"\nname = \"ops\"\nenv = [\"PATH\", \"x\"]\n");
        assert!(def.is_none());
        let said_all = errors(&problems);
        for said in ["description is required", "harness is required", "\"daemon\" is not persistent", "not its directory's name", "env PATH", "env \"x\""] {
            assert!(said_all.iter().any(|e| e.contains(said)), "{said}: {said_all:?}");
        }
        let (_, problems) = parse("ops", "name = \"ops\"\ndescription = \"x\"\nkind = \"persistent\"\nharness = \"pi\"\n");
        assert_eq!(errors(&problems), ["a persistent bot needs cwd, where it works"]);
        let (def, problems) = parse("ops", "name = \"ops\"\ndescription = \"x\"\nkind = \"oneoff\"\nharness = \"pi\"\n");
        assert_eq!((def.is_some(), problems.len()), (true, 0));
        let (_, problems) = parse("ops", "name = 3\ndescription = \"\"\nkind = \"oneoff\"\nharness = \"pi\"\nargs = \"-x\"\n");
        assert_eq!(errors(&problems), ["name must be a string", "description is required, and empty", "args must be a list of strings"]);
    }

    #[test]
    fn unknown_fields_only_warn() {
        let (def, problems) = parse("trainer", &format!("{TRAINER}colour = \"red\"\n"));
        assert!(def.is_some());
        assert_eq!(problems, [warning("unknown field colour, left alone".into())]);
    }

    #[test]
    fn a_syntax_error_says_its_line() {
        let (def, problems) = parse("broken", "name = \"broken\"\nkind = \n");
        assert!(def.is_none());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.starts_with("bot.toml line 2: "), "{:?}", problems[0]);
    }

    #[test]
    fn new_bots_are_listed_and_never_written_over() {
        let home = crate::test_dir("bots");
        let bots = home.join("bots");
        let dir = create(&bots, "trainer", BotKind::Persistent, Some("codex")).unwrap();
        for f in ["bot.toml", "template.md", "AGENTS.md"] {
            assert!(dir.join(f).is_file(), "{f}");
        }
        assert!(matches!(create(&bots, "trainer", BotKind::Oneoff, None), Err(BotError::Exists(..))));
        assert!(matches!(create(&bots, "Trainer", BotKind::Oneoff, None), Err(BotError::BadName(_))));
        // Fresh: only cwd is missing (and the description is a placeholder).
        let bot = find(&bots, "trainer").unwrap();
        assert_eq!(errors(&bot.problems), ["a persistent bot needs cwd, where it works"]);
        let toml = dir.join("bot.toml");
        let text = std::fs::read_to_string(&toml).unwrap() + "cwd = \"/tmp/example\"\n";
        std::fs::write(&toml, text).unwrap();
        let bot = find(&bots, "trainer").unwrap();
        assert!(!bot.invalid(), "{:?}", bot.problems);
        assert_eq!(bot.def.unwrap().harness, "codex");
        // A oneoff one needs no cwd; with no --harness, it needs one.
        create(&bots, "ops", BotKind::Oneoff, None).unwrap();
        assert_eq!(errors(&find(&bots, "ops").unwrap().problems), ["harness is required, and empty"]);
        // A broken one is listed with the others.
        std::fs::create_dir(bots.join("broken")).unwrap();
        std::fs::create_dir(bots.join(".git")).unwrap();
        std::fs::write(bots.join("notes.txt"), "").unwrap();
        let names: Vec<_> = list(&bots).unwrap().into_iter().map(|b| (b.name, b.def.is_some())).collect();
        assert_eq!(names, [("broken".to_owned(), false), ("ops".into(), false), ("trainer".into(), true)]);
        assert!(matches!(find(&bots, "nova"), Err(BotError::NotFound(_))));
        assert!(list(&home.join("nothing")).unwrap().is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
}
