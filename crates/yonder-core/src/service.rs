//! The launchd plist that keeps yonderd running: started at login, started
//! again if it dies, with an explicit short PATH and no shell profile.

use std::path::{Path, PathBuf};

/// The PATH yonderd gets: where hc is, then the system's.
const SYSTEM_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// `SYSTEM_PATH`, after the directory of `program` if it has one.
pub fn path_with(program: Option<&Path>) -> String {
    match program.and_then(Path::parent).filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) if !SYSTEM_PATH.split(':').any(|d| Path::new(d) == dir) => format!("{}:{SYSTEM_PATH}", dir.display()),
        _ => SYSTEM_PATH.into(),
    }
}

/// `program` itself if it is a path, else where PATH has it.
pub fn find_program(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return Some(program.into());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file())
}

/// The plist of `label`, running `program` with exactly `env`; its own
/// output goes to `log`.
pub fn plist(label: &str, program: &Path, env: &[(String, String)], log: &Path) -> String {
    let string = |s: &str| format!("<string>{}</string>", xml(s));
    let env: String = env.iter().map(|(k, v)| format!("\t\t<key>{}</key>\n\t\t{}\n", xml(k), string(v))).collect();
    let log = string(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	{label}
	<key>ProgramArguments</key>
	<array>
		{program}
	</array>
	<key>EnvironmentVariables</key>
	<dict>
{env}	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardOutPath</key>
	{log}
	<key>StandardErrorPath</key>
	{log}
</dict>
</plist>
"#,
        label = string(label),
        program = string(&program.display().to_string()),
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plists_run_yonderd_with_a_short_path() {
        let env = [("PATH".into(), path_with(Some(Path::new("/tmp/example/bin/hc")))), ("HOME".into(), "/tmp/example".into())];
        let p = plist("dev.yonder.daemon", Path::new("/tmp/example/bin/yonderd"), &env, Path::new("/tmp/example/s/logs/yonderd.out"));
        assert!(p.contains("<key>Label</key>\n\t<string>dev.yonder.daemon</string>"), "{p}");
        assert!(p.contains("<string>/tmp/example/bin/yonderd</string>"));
        assert!(p.contains("<string>/tmp/example/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>"));
        assert!(p.contains("<key>KeepAlive</key>\n\t<true/>") && p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(!p.contains("zsh") && !p.contains(".zshrc") && !p.contains("-lic"), "no login shell");
        let odd = plist("a&b", Path::new("/x/<y>"), &[], Path::new("/l"));
        assert!(odd.contains("<string>a&amp;b</string>") && odd.contains("<string>/x/&lt;y&gt;</string>"));
    }

    #[test]
    fn paths_put_hc_first_once() {
        assert_eq!(path_with(None), SYSTEM_PATH);
        assert_eq!(path_with(Some(Path::new("hc"))), SYSTEM_PATH);
        assert_eq!(path_with(Some(Path::new("/usr/bin/hc"))), SYSTEM_PATH);
        assert!(path_with(Some(Path::new("/tmp/example/bin/hc"))).starts_with("/tmp/example/bin:"));
        assert_eq!(find_program(Path::new("/tmp/example/hc")), Some("/tmp/example/hc".into()));
        assert_eq!(find_program(Path::new("sh")).map(|p| p.is_file()), Some(true));
    }
}
