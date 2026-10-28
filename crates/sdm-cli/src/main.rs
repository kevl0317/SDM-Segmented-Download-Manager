//! Headless CLI: download a URL with a live text progress bar.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use sdm_core::{default_download_dir, human_bytes, human_time, Download, Status};

#[derive(Parser)]
#[command(name = "sdm", about = "Segmented download manager (CLI).")]
struct Args {
    /// URL to download.
    url: String,
    /// Destination directory (default: ~/Downloads).
    #[arg(short = 'o', long = "out")]
    out: Option<String>,
    /// Number of parallel connections.
    #[arg(short = 'n', long = "connections", default_value_t = 8)]
    connections: usize,
    /// Override the output filename.
    #[arg(short = 'f', long = "filename")]
    filename: Option<String>,
}

fn render(d: &Download) {
    let pct = d.progress();
    let done = human_bytes(Some(d.downloaded()));
    let total = human_bytes(d.size());
    let speed = format!("{}/s", human_bytes(Some(d.speed())));
    let bar = match pct {
        None => format!("{} ({} conns, size unknown)", done, d.connections()),
        Some(p) => {
            let width = 30usize;
            let filled = ((width as f64 * p) as usize).min(width);
            format!(
                "[{}{}] {:5.1}%",
                "#".repeat(filled),
                "-".repeat(width - filled),
                p * 100.0
            )
        }
    };
    print!(
        "\r{}  {}/{}  {:>12}  ETA {}   ",
        bar,
        done,
        total,
        speed,
        human_time(d.eta())
    );
    let _ = std::io::stdout().flush();
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let out = args
        .out
        .unwrap_or_else(|| default_download_dir().to_string_lossy().into_owned());

    let d = Arc::new(Download::new(
        &args.url,
        out.as_str(),
        args.filename.as_deref(),
        args.connections,
        None,
    ));
    println!("URL: {}", args.url);

    let worker = d.clone();
    let handle = tokio::spawn(async move { worker.run().await });

    let mut interrupted = false;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nPausing...");
                d.pause();
                interrupted = true;
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if d.status() == Status::Running {
                    render(&d);
                }
                if !d.is_active() {
                    break;
                }
            }
        }
    }

    let _ = handle.await;

    if interrupted {
        println!(
            "Paused at {}. Re-run to resume.",
            human_bytes(Some(d.downloaded()))
        );
        return ExitCode::from(130);
    }

    render(&d);
    println!();
    if d.status() == Status::Complete {
        println!("Done -> {}", d.output_path().display());
        println!(
            "Ranges: {}  Segments: {}  Size: {}",
            if d.supports_ranges() { "yes" } else { "no" },
            d.segment_count().max(1),
            human_bytes(d.size())
        );
        ExitCode::SUCCESS
    } else {
        println!(
            "Failed: {}",
            d.error().unwrap_or_else(|| d.status().as_str().to_string())
        );
        ExitCode::FAILURE
    }
}
