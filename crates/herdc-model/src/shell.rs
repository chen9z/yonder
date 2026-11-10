//! The shell agents start in on a core's machine: its own, with login and
//! interactive profiles loaded (the viewer's shell means nothing there).
//!
//! From herdr-app `src/remote_shell.rs`, Apache-2.0.

pub fn check(commands: &[String]) -> Vec<String> {
    let posix = "for c; do unalias -- \"$c\" 2>/dev/null; command -v \"$c\" >/dev/null 2>&1 && printf 'herdr-has:%s\\n' \"$c\"; done";
    in_shell(posix, "for c in $argv[2..]; command -v $c >/dev/null 2>&1; and printf 'herdr-has:%s\\n' $c; end", posix, commands.to_vec())
}

pub fn launch(command: Vec<String>) -> Vec<String> {
    in_shell("\"$@\"; exec \"$0\" -li", "command $argv[2..]; exec $argv[1] -li", "\"$@\"; exec \"${SHELL:-sh}\" -i", command)
}

/// Select the shell there, not the viewer's shell. The core may have started
/// with only the SSH session's minimal PATH; load login and interactive profiles
/// before checking or launching. Only our fixed scripts become shell source.
fn in_shell(posix: &str, fish: &str, fallback: &str, args: Vec<String>) -> Vec<String> {
    let quote = |s: &str| s.replace('\'', "'\\''");
    let script = format!(
        r#"shell=${{SHELL:-/bin/sh}}
case "${{shell##*/}}" in
  fish) exec "$shell" -lic '{}' "$shell" "$@" ;;
  sh|bash|zsh|dash|ksh) exec "$shell" -lic '{}' "$shell" "$@" ;;
  *) exec sh -c '{}' sh "$@" ;;
esac"#,
        quote(fish),
        quote(posix),
        quote(fallback),
    );
    // Keep the command at argv[4..]: the Windows fallback reads it there when
    // sh is unavailable. Passing it separately also preserves literal arguments.
    let mut argv = vec!["sh".into(), "-c".into(), script, "sh".into()];
    argv.extend(args);
    argv
}
