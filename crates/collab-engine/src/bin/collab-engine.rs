use std::env;
use std::io::{self, BufReader, Write};

use collab_engine::frame::{read_frame, write_frame};
use collab_engine::limits::Limits;
use collab_engine::outcome::{EngineReport, EngineStatus, LimitKind};
use collab_engine::process::apply_rlimits_now;
use collab_engine::protocol::{preflight_wire_json, Request};
use collab_engine::CollabEngine;

fn main() {
    scrub_inherited_env();
    let mut limits = Limits::default();
    #[cfg(feature = "test-hang")]
    let mut dump_rlimits = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--max-input" => limits.max_input_bytes = parse_u64(&require_arg(&mut args)),
            "--max-output" => limits.max_output_bytes = parse_u64(&require_arg(&mut args)),
            "--max-load" => limits.max_load_bytes = parse_u64(&require_arg(&mut args)),
            "--max-frame" => limits.max_frame_bytes = parse_u64(&require_arg(&mut args)),
            "--max-tail" => limits.max_tail_updates = parse_u64(&require_arg(&mut args)) as usize,
            "--max-ops" => limits.max_ops = parse_u64(&require_arg(&mut args)) as u32,
            "--timeout-ms" => limits.timeout_ms = parse_u64(&require_arg(&mut args)),
            "--max-as" => limits.max_child_as_bytes = parse_u64(&require_arg(&mut args)),
            "--max-observed-rss" => {
                limits.max_observed_rss_bytes = parse_u64(&require_arg(&mut args))
            }
            "--max-stack" => limits.max_child_stack_bytes = parse_u64(&require_arg(&mut args)),
            #[cfg(feature = "test-hang")]
            "--test-hang-ms" => {
                let ms = parse_u64(&require_arg(&mut args));
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
            #[cfg(feature = "test-hang")]
            "--dump-rlimits" => dump_rlimits = true,
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
    if let Err(err) = apply_rlimits_now(
        limits.max_child_as_bytes,
        cpu_secs,
        limits.max_child_stack_bytes,
    ) {
        eprintln!("setrlimit failed: {err}");
        std::process::exit(2);
    }
    #[cfg(feature = "test-hang")]
    if dump_rlimits {
        print_applied_rlimits();
        return;
    }

    let mut engine = CollabEngine::new(limits);
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();
    loop {
        match read_frame(&mut reader, limits.max_frame_bytes) {
            Ok(None) => break,
            Ok(Some(buf)) => {
                let report = match serde_json::from_slice::<serde_json::Value>(&buf) {
                    Ok(v) => match preflight_wire_json(&v, &limits) {
                        Err(outcome) => EngineReport::new(outcome),
                        Ok(()) => match serde_json::from_value::<Request>(v) {
                            Ok(req) => EngineReport::new(engine.handle(&req)),
                            Err(err) => EngineReport::new(EngineStatus::Malformed {
                                detail: format!("request json: {err}"),
                            }),
                        },
                    },
                    Err(err) => EngineReport::new(EngineStatus::Malformed {
                        detail: format!("request json: {err}"),
                    }),
                };
                if let Err(err) = write_report(&mut stdout, &report, limits.max_frame_bytes) {
                    eprintln!("write response: {err}");
                    break;
                }
            }
            Err(err) => {
                let report = EngineReport::new(err.into_status(&limits));
                let _ = write_report(&mut stdout, &report, limits.max_frame_bytes);
                if matches!(
                    report.outcome,
                    EngineStatus::ResourceLimit {
                        kind: LimitKind::Frame,
                        ..
                    }
                ) {
                    break;
                }
            }
        }
    }
}

fn write_report<W: Write>(w: &mut W, report: &EngineReport, max: u64) -> Result<(), String> {
    let payload = serde_json::to_vec(report).map_err(|e| e.to_string())?;
    if payload.len() as u64 > max {
        return Err("response exceeds frame bound".into());
    }
    write_frame(w, &payload, max).map_err(|e| e.to_string())
}

#[cfg(feature = "test-hang")]
fn print_applied_rlimits() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut as_lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_AS, &mut as_lim) == 0 {
            println!("RLIMIT_AS={}", as_lim.rlim_cur);
        }
        let mut st_lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_STACK, &mut st_lim) == 0 {
            println!("RLIMIT_STACK={}", st_lim.rlim_cur);
        }
    }
    match std::env::var("DATABASE_APP_URL") {
        Ok(_) => println!("DATABASE_APP_URL=set"),
        Err(_) => println!("DATABASE_APP_URL=unset"),
    }
    println!("environ_count={}", std::env::vars_os().count());
}

fn scrub_inherited_env() {
    let keys: Vec<_> = std::env::vars_os().map(|(k, _)| k).collect();
    for key in keys {
        std::env::remove_var(key);
    }
}

fn require_arg(args: &mut impl Iterator<Item = String>) -> String {
    args.next().unwrap_or_else(|| usage())
}

fn parse_u64(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| usage())
}

fn usage() -> ! {
    eprintln!(
        "usage: collab-engine [--max-input N] [--max-output N] [--max-load N] [--max-frame N] [--max-tail N] [--max-ops N] [--timeout-ms N] [--max-as N] [--max-observed-rss N] [--max-stack N]"
    );
    std::process::exit(2);
}
