use std::io::{self, BufWriter, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use exg_wal_dump::dump;

fn print_usage() {
    eprintln!("Usage: exg-wal-dump --wal-dir <path> [--from-seq <N>]");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut wal_dir: Option<PathBuf> = None;
    let mut from_seq: u64 = 0;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--wal-dir" => {
                i += 1;
                if i >= args.len() {
                    print_usage();
                    return ExitCode::from(2);
                }
                wal_dir = Some(PathBuf::from(&args[i]));
            }
            "--from-seq" => {
                i += 1;
                if i >= args.len() {
                    print_usage();
                    return ExitCode::from(2);
                }
                match args[i].parse() {
                    Ok(n) => from_seq = n,
                    Err(e) => {
                        eprintln!("--from-seq: {e}");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_usage();
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let Some(dir) = wal_dir else {
        print_usage();
        return ExitCode::from(2);
    };

    let stdout = io::stdout().lock();
    let mut out = BufWriter::new(stdout);
    if let Err(e) = dump(&dir, from_seq, &mut out) {
        out.flush().ok();
        eprintln!("exg-wal-dump: {e:#}");
        return ExitCode::from(1);
    }
    if let Err(e) = out.flush() {
        eprintln!("exg-wal-dump: stdout flush: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
