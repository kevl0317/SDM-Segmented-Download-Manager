// Stands in for explorer.exe beside the app under test: records what the app asked
// to open, so testing Open and Project page opens no real window.
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(log) = std::env::var_os("SDM_TEST_OPEN_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{}", args.join(" "));
        }
    }
}
