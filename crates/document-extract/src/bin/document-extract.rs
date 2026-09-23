use std::env;
use std::io::{self, Read, Write};

use document_extract::limits::Limits;
use document_extract::parse::extract_bytes;
use document_extract::process::apply_rlimits_now;

fn main() {
    let mut name = String::from("document.hwp");
    let mut limits = Limits::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => {
                name = args.next().unwrap_or_else(|| usage());
            }
            "--max-input" => {
                limits.max_input_bytes = parse_u64(&args.next().unwrap_or_else(|| usage()));
            }
            "--max-output" => {
                limits.max_output_chars =
                    parse_u64(&args.next().unwrap_or_else(|| usage())) as usize;
            }
            "--max-zip-entries" => {
                limits.max_zip_entries =
                    parse_u64(&args.next().unwrap_or_else(|| usage())) as usize;
            }
            "--max-zip-uncompressed" => {
                limits.max_zip_uncompressed_bytes =
                    parse_u64(&args.next().unwrap_or_else(|| usage()));
            }
            "--timeout-ms" => {
                limits.timeout_ms = parse_u64(&args.next().unwrap_or_else(|| usage()));
            }
            "--max-rss" => {
                limits.max_child_rss_bytes = parse_u64(&args.next().unwrap_or_else(|| usage()));
            }
            #[cfg(feature = "test-hang")]
            "--test-hang-ms" => {
                let ms = parse_u64(&args.next().unwrap_or_else(|| usage()));
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
            "--help" | "-h" => usage(),
            other => {
                eprintln!("unknown arg {other}");
                usage();
            }
        }
    }

    if let Err(err) = limits.validate() {
        eprintln!("invalid limits: {err}");
        std::process::exit(2);
    }
    let cpu_secs = (limits.timeout_ms / 1000).max(1);
    if let Err(err) = apply_rlimits_now(limits.max_child_rss_bytes, cpu_secs) {
        eprintln!("setrlimit failed: {err}");
        std::process::exit(2);
    }

    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes).expect("read stdin");
    let report = extract_bytes(&bytes, &name, &limits);
    let json = serde_json::to_vec(&report).expect("serialize report");
    io::stdout().write_all(&json).expect("write stdout");
    io::stdout().write_all(b"\n").ok();
}

fn parse_u64(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| usage())
}

fn usage() -> ! {
    eprintln!(
        "usage: document-extract --name <file.hwp|file.hwpx> [--max-input N] [--max-output N] [--max-zip-entries N] [--max-zip-uncompressed N] [--timeout-ms N] [--max-rss N] < bytes"
    );
    std::process::exit(2);
}
