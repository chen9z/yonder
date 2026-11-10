//! The whole way in, against the test computer (an in-process SSH server
//! that runs commands here) and the isolated herdr-core behind it.

use herdc_ssh::{generate_key, greet, Credential, Error, Pipe, Target};
use herdc_testkit::computer::{self, Options};
use herdc_testkit::Session as CoreSession;
use serde_json::json;
use tokio::io::AsyncReadExt;

const USER: &str = "me";
const PASSWORD: &str = "open sesame";

/// Serves `core` on a free port; returns the target and the server's fingerprint.
async fn serve(core: &CoreSession, keyboard_only: bool) -> (Target, String) {
    let options = Options { user: USER.into(), password: PASSWORD.into(), keyboard_only, ..Options::default() };
    let served = computer::serve(core, options).await.unwrap();
    (Target { host: "127.0.0.1".into(), port: served.port, user: USER.into() }, served.fingerprint)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_reaches_the_core_through_herdrs_own_pipe() {
    let core = CoreSession::new();
    let (target, fingerprint) = serve(&core, false).await;
    let greeted = greet(&target, None).await.unwrap();
    assert_eq!(greeted.host_key.fingerprint, fingerprint);
    assert_eq!(greeted.host_key.algorithm, "ssh-ed25519");
    let ssh = greeted.login(USER, &Credential::Password(PASSWORD.into())).await.unwrap();
    let client = ssh.connect_core(&Pipe::default()).await.unwrap();
    herdc_wire::compat::check(client.hello()).unwrap();
    let list = client.call("terminal.list", json!({})).await.unwrap();
    assert_eq!(list["terminals"], json!([]));

    // The SSH session ending ends the core connection too.
    ssh.close().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), client.closed()).await.expect("closed with ssh");
}

#[tokio::test(flavor = "multi_thread")]
async fn macos_style_keyboard_interactive_takes_the_password() {
    let core = CoreSession::new();
    let (target, _) = serve(&core, true).await;
    let ssh = greet(&target, None).await.unwrap().login(USER, &Credential::Password(PASSWORD.into())).await.unwrap();
    let exec = ssh.exec("printf ok").await.unwrap();
    let mut out = String::new();
    let mut stdout = exec.stdout;
    stdout.read_to_string(&mut out).await.unwrap();
    assert_eq!(out, "ok");

    let wrong = greet(&target, None).await.unwrap().login(USER, &Credential::Password("nope".into())).await;
    assert!(matches!(wrong, Err(Error::WrongPassword)), "a wrong password must be said so");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_installed_key_logs_in_and_a_changed_host_key_stops_before_login() {
    let core = CoreSession::new();
    let (target, fingerprint) = serve(&core, false).await;
    let (private, public) = generate_key("herdc@test").unwrap();
    let credential = Credential::Key { openssh: private, passphrase: None };
    assert!(greet(&target, Some(&fingerprint)).await.unwrap().login(USER, &credential).await.is_err(), "not installed yet");

    // As the phone installs it: once in by password, append to authorized_keys.
    let ssh = greet(&target, Some(&fingerprint)).await.unwrap().login(USER, &Credential::Password(PASSWORD.into())).await.unwrap();
    let line = format!("mkdir -p ~/.ssh && printf '%s\\n' '{public}' >> ~/.ssh/authorized_keys");
    let exec = ssh.exec(&line).await.unwrap();
    let mut out = Vec::new();
    let mut stdout = exec.stdout;
    stdout.read_to_end(&mut out).await.unwrap();
    greet(&target, Some(&fingerprint)).await.unwrap().login(USER, &credential).await.unwrap();

    let changed = greet(&target, Some("SHA256:somethingelse")).await;
    assert!(matches!(changed, Err(Error::HostKeyChanged { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_without_herdr_says_so() {
    let core = CoreSession::new();
    let (target, _) = serve(&core, false).await;
    let ssh = greet(&target, None).await.unwrap().login(USER, &Credential::Password(PASSWORD.into())).await.unwrap();
    // Herdr's own line, with a PATH and HOME where there is no herdr-core.
    let line = format!("env -i PATH=/usr/bin:/bin HOME=/nonexistent {}", Pipe::default().line());
    let result = ssh.connect_core(&Pipe::Custom(line)).await;
    assert!(matches!(result, Err(Error::NotInstalled)), "{:?}", result.err());
}

#[tokio::test(start_paused = true)]
async fn something_that_takes_the_line_and_says_nothing_is_unreachable() {
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = Target { host: "127.0.0.1".into(), port: silent.local_addr().unwrap().port(), user: USER.into() };
    let greeted = greet(&target, None).await;
    assert!(matches!(greeted, Err(Error::Unreachable(..))), "{:?}", greeted.err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_line_ends_what_it_ran_as_sshd_does() {
    let core = CoreSession::new();
    let (target, _) = serve(&core, false).await;
    let login = async || greet(&target, None).await.unwrap().login(USER, &Credential::Password(PASSWORD.into())).await.unwrap();
    // Two lines at once, each running a program on its first channel.
    let (one, two) = (login().await, login().await);
    let pid = core.path("one.pid");
    let mut first = one.exec(&format!("echo $$ > {}; exec cat", pid.display())).await.unwrap();
    let mut second = two.exec("cat").await.unwrap();
    use tokio::io::AsyncWriteExt;
    second.stdin.write_all(b"second\n").await.unwrap();
    let mut echoed = [0u8; 7];
    second.stdout.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"second\n", "each line's channel reaches its own program");
    first.stdin.write_all(b"x\n").await.unwrap();
    let mut x = [0u8; 2];
    first.stdout.read_exact(&mut x).await.unwrap();
    let pid = std::fs::read_to_string(&pid).unwrap().trim().to_owned();

    one.close().await;
    for _ in 0..100 {
        let alive = std::process::Command::new("kill").args(["-0", &pid]).status().unwrap().success();
        if !alive {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the program outlived its line");
}
