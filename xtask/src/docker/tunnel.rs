//! The profile `tunnel` of `docker/compose.yaml`, as compose reads it: with a token it
//! is cloudflared on the network `edge` holding the token as a secret, fvoci's port
//! stays on the loopback unless `.env` says otherwise, and without the profile none of
//! it is there. A tunnel dials Cloudflare, which a trial must not do:
//! cloudflared is run once with a token that is no token, which it reads from its
//! secret's file and refuses before it dials anything.

use anyhow::{Context as _, Result, ensure};
use serde_json::Value;

use super::Trial;
use crate::steps::say;

/// The service and the secret the profile adds.
const SERVICE: &str = "cloudflared";
const SECRET: &str = "tunnel_token";

/// Checks the profile on and off.
///
/// # Errors
///
/// Fails naming what compose made of the profile that it should not have.
pub(super) fn profile(trial: &Trial) -> Result<()> {
    let token = format!("fake-tunnel-token-{}", super::hex(&super::random::<16>()?));
    let with = config(trial, &["--profile", "tunnel"], &token)?;
    ensure!(
        !with.to_string().contains(&token),
        "compose's reading of the profile shows the token"
    );
    let service = &with["services"][SERVICE];
    let image = service["image"].as_str().unwrap_or_default();
    let version = image
        .strip_prefix("cloudflare/cloudflared:")
        .unwrap_or_default();
    ensure!(
        !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'),
        "cloudflared's image is not one pinned version: {image:?}"
    );
    let networks: Vec<&String> = service["networks"]
        .as_object()
        .map(|networks| networks.keys().collect())
        .unwrap_or_default();
    ensure!(
        networks == ["edge"],
        "cloudflared is on {networks:?}, not on edge alone"
    );
    let secrets: Vec<&str> = service["secrets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|secret| secret["source"].as_str())
        .collect();
    ensure!(
        secrets == [SECRET] && with["secrets"][SECRET]["environment"] == "FVOCI_TUNNEL_TOKEN",
        "cloudflared's secrets are {secrets:?}, not the token from FVOCI_TUNNEL_TOKEN"
    );
    ensure!(
        service["ports"].is_null(),
        "cloudflared publishes a port of the host"
    );
    // Beside the tunnel, edge's range is trusted, and so is whatever reaches fvoci's port.
    let published: Vec<&str> = with["services"]["fvoci"]["ports"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|port| port["host_ip"].as_str().unwrap_or_default())
        .collect();
    ensure!(
        published == ["127.0.0.1"],
        "with the profile, fvoci's port is published on {published:?}, not on the loopback alone"
    );

    let without = config(trial, &[], &token)?;
    ensure!(
        without["services"].get(SERVICE).is_none() && without["secrets"].get(SECRET).is_none(),
        "without the profile, compose still has cloudflared or its secret"
    );
    refuses_a_fake_token(trial, &token)?;
    say("    with the profile: cloudflared on edge with its secret; without: nothing of it");
    Ok(())
}

/// What cloudflared says of a token it cannot read as one.
const INVALID_TOKEN: &str = "Provided Tunnel token is not valid";

/// Runs the profile's cloudflared once with `token`: it must read the secret's file (an
/// unreadable or empty one says something else) and refuse what it holds.
fn refuses_a_fake_token(trial: &Trial, token: &str) -> Result<()> {
    let arguments = ["--profile", "tunnel", "run", "--rm", "--no-deps", SERVICE];
    let output = trial
        .command(&arguments)
        .env("FVOCI_TUNNEL_TOKEN", token)
        .output()
        .context("starting docker compose run")?;
    let said = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        !said.contains(token),
        "cloudflared shows the token it was given"
    );
    ensure!(
        !output.status.success() && said.contains(INVALID_TOKEN),
        "cloudflared did not read and refuse the fake token from its secret ({}): {}",
        output.status,
        said.trim()
    );
    Ok(())
}

/// What `docker compose config` makes of the trial's file with `profiles` and a token.
fn config(trial: &Trial, profiles: &[&str], token: &str) -> Result<Value> {
    let mut arguments = profiles.to_vec();
    arguments.extend(["config", "--format", "json"]);
    let output = trial
        .command(&arguments)
        .env("FVOCI_TUNNEL_TOKEN", token)
        .output()
        .context("starting docker compose config")?;
    ensure!(
        output.status.success(),
        "docker compose {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("docker compose config gave no JSON")
}
