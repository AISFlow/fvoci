//! `fvoci-server healthcheck [worker|compact|thumbnail]` (source `cli.ts`
//! `healthcheck`): exit 0 when the local server's `GET /ready` answers 2xx
//! within 4 s, 1 otherwise.
//!
//! It reads only `FVOCI_BIND` (the server's listen address) and never loads
//! the server config, database URL or keys. The split worker roles the
//! source probed through heartbeat files run inside the server here, so
//! those modes always report unhealthy with a message.

use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

pub const HEALTHCHECK_ARG: &str = "healthcheck";

/// Source `AbortSignal.timeout(4_000)`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

const USAGE: &str = "usage: fvoci-server healthcheck [worker|compact|thumbnail]";

/// `Some(exit code)` when this process was started as `healthcheck`.
pub fn maybe_run() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HEALTHCHECK_ARG)) {
        return None;
    }
    let mode = args.next();
    Some(run(mode, std::env::var("FVOCI_BIND").ok()))
}

fn run(mode: Option<OsString>, bind: Option<String>) -> i32 {
    if let Some(mode) = mode {
        match mode.to_str() {
            Some(role @ ("worker" | "compact" | "thumbnail")) => {
                eprintln!(
                    "fvoci-server healthcheck {role}: no separate {role} process; \
                     the server runs it in-process (probe without a mode)"
                );
            }
            _ => eprintln!("{USAGE}"),
        }
        return 1;
    }
    let target = match probe_target(bind.as_deref()) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("fvoci-server healthcheck: {message}");
            return 1;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("fvoci-server healthcheck: runtime: {err}");
            return 1;
        }
    };
    match runtime.block_on(probe_ready(target)) {
        Ok(status) if status.is_success() => 0,
        Ok(status) => {
            eprintln!(
                "fvoci-server healthcheck: /ready answered {}",
                status.as_u16()
            );
            1
        }
        Err(err) => {
            eprintln!("fvoci-server healthcheck: {err}");
            1
        }
    }
}

/// The listen address as a connectable one: a wildcard bind is probed on
/// the same family's loopback.
fn probe_target(bind: Option<&str>) -> Result<SocketAddr, String> {
    let raw = bind.unwrap_or("127.0.0.1:0");
    let mut addr: SocketAddr = raw
        .parse()
        .map_err(|err| format!("invalid FVOCI_BIND: {err}"))?;
    if addr.port() == 0 {
        return Err("FVOCI_BIND has no fixed port to probe".to_string());
    }
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    Ok(addr)
}

async fn probe_ready(target: SocketAddr) -> Result<reqwest::StatusCode, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|err| format!("client: {err}"))?;
    let url = format!("http://{target}{}", crate::http::probes::READY_PATH);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("GET /ready: {}", err.without_url()))?;
    // Only the status matters; dropping the response closes the body.
    Ok(response.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_bind_probes_loopback() {
        assert_eq!(
            probe_target(Some("0.0.0.0:8080")).unwrap(),
            "127.0.0.1:8080".parse().unwrap()
        );
        assert_eq!(
            probe_target(Some("[::]:8080")).unwrap(),
            "[::1]:8080".parse().unwrap()
        );
        assert_eq!(
            probe_target(Some("10.0.0.5:9000")).unwrap(),
            "10.0.0.5:9000".parse().unwrap()
        );
    }

    #[test]
    fn unprobeable_bind_is_an_error() {
        assert!(probe_target(None).is_err());
        assert!(probe_target(Some("127.0.0.1:0")).is_err());
        assert!(probe_target(Some("not-an-addr")).is_err());
    }

    #[test]
    fn split_roles_and_unknown_modes_exit_one() {
        for mode in ["worker", "compact", "thumbnail", "bogus"] {
            assert_eq!(
                run(Some(mode.into()), Some("127.0.0.1:1".into())),
                1,
                "{mode}"
            );
        }
    }
}
