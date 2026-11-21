// Counts a folder with the same code the program uses. For checking against `du` and for timing.
//   cargo run --release --example usage -- [--apparent] [--cross] [--threads N] [--flat] PATH
use std::path::PathBuf;
use std::time::Instant;

use rust_commander::usage::{self, Mode, Options};

fn main() {
    let mut mode = Mode::DISK;
    let mut threads = None;
    let mut flat = false;
    let mut root = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--apparent" => mode.apparent = true,
            "--cross" => mode.same_device = false,
            "--flat" => flat = true,
            "--threads" => threads = args.next().and_then(|n| n.parse().ok()),
            other => root = Some(PathBuf::from(other)),
        }
    }
    let Some(root) = root else {
        eprintln!("usage: usage [--apparent] [--cross] [--threads N] [--flat] PATH");
        std::process::exit(2);
    };
    let threads = threads.unwrap_or_else(|| usage::threads_for(usage::medium_of(&root), usage::cores()));
    let start = Instant::now();
    let opts = Options { root, mode, threads };
    let report = match usage::run(&opts, &|| false, &mut |_| {}) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("{}: {err}", opts.root.display());
            std::process::exit(1);
        }
    };
    if flat {
        println!("{}", report.total);
    } else {
        for (child, bytes) in report.children.iter().zip(&report.bytes) {
            println!("{bytes}\t{}", child.name.to_string_lossy());
        }
        println!("{}\ttotal", report.total);
        eprintln!(
            "{} entries, {} unreadable, {} other mounts, {} threads, {:.3}s",
            report.entries,
            report.unreadable,
            report.skipped_mounts,
            threads,
            start.elapsed().as_secs_f64()
        );
    }
}
