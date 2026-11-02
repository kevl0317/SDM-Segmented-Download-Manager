//! The fault server as a program, for driving the GUI by hand or from a script.
//! `sdm-faultserver [port]` serves the URLs described in the library's docs, and
//! reads commands from stdin: `down`, `silent`, `up`, `replace`, `log`, `quit`.
//! `sdm-faultserver --verify <file> <seed> <size>` checks a downloaded file.

use std::process::ExitCode;

use sdm_testkit::{verify, FaultServer};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--verify") {
        let number = |i: usize| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
        return match verify(std::path::Path::new(&args[1]), number(2), number(3)) {
            Ok(()) => {
                println!("OK");
                ExitCode::SUCCESS
            }
            Err(e) => {
                println!("BAD: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let port = args.first().and_then(|p| p.parse().ok()).unwrap_or(0);
    let server = FaultServer::start_on(port);
    println!("{}", server.port());
    let mut line = String::new();
    while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
        match line.trim() {
            "down" => server.go_down(),
            "silent" => server.go_silent(),
            "up" => server.come_back(),
            "replace" => server.replace_files(),
            "log" => {
                for r in server.log() {
                    println!("{r:?}");
                }
                println!("--");
            }
            "quit" => break,
            _ => {}
        }
        line.clear();
    }
    ExitCode::SUCCESS
}
