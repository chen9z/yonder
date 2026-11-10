//! A computer to point the phone at: SSH on the LAN, a password, and an
//! isolated herdr-core behind it (never the user's). Stops, and removes
//! the session, on Ctrl-C.
//!
//! herdc-test-computer [--listen 0.0.0.0:2222] [--user me] [--password 'open sesame'] [--keyboard-only] [--host-key FILE]

use herdc_testkit::computer::{self, Options};
use herdc_testkit::Session;

#[tokio::main]
async fn main() {
    let mut options = Options { listen: "0.0.0.0:2222".into(), ..Options::default() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => options.listen = args.next().expect("--listen ADDR"),
            "--user" => options.user = args.next().expect("--user NAME"),
            "--password" => options.password = args.next().expect("--password WORDS"),
            "--keyboard-only" => options.keyboard_only = true,
            "--host-key" => options.host_key = Some(args.next().expect("--host-key FILE").into()),
            other => panic!("unknown argument {other:?}"),
        }
    }
    let session = Session::new();
    let (user, password) = (options.user.clone(), options.password.clone());
    let served = computer::serve(&session, options).await.expect("listen");
    println!("port {}", served.port);
    println!("user {user}");
    println!("password {password}");
    println!("fingerprint {}", served.fingerprint);
    println!("session {}", session.root.display());
    println!("ready");
    let _ = tokio::signal::ctrl_c().await;
}
