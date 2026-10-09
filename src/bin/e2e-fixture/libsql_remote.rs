//! Hosted-only adapter. All returned errors are fixed codes; SDK causes never
//! reach diagnostic JSON, browser logs or Actions output.
use fvoci_server::auth::password::{hash_password, Keyring};
use fvoci_server::db::{
    backend::{Backend, RemoteDatabase},
    e2e_fixture,
};
use serde_json::{json, Value};
use std::io::Read;
use uuid::Uuid;

fn required(name: &str) -> Result<String, &'static str> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or("TURSO_UI_INPUT_REQUIRED")
}

pub async fn run() -> Result<(), &'static str> {
    if required("FVOCI_E2E_TURSO_UI_SELECTED")? != "1" {
        return Err("TURSO_UI_ALLOCATION_REQUIRED");
    }
    let mode = std::env::args().nth(1).ok_or("TURSO_UI_MODE_REQUIRED")?;
    if !matches!(mode.as_str(), "baseline" | "owner" | "member" | "observe") {
        return Err("TURSO_UI_MODE_INVALID");
    }
    if matches!(mode.as_str(), "owner" | "member")
        && (required("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE")? != "true"
            || required("FVOCI_TEST_TURSO_DESTRUCTIVE")? != "true")
    {
        return Err("TURSO_UI_MUTATION_NOT_GRANTED");
    }
    let remote = RemoteDatabase::connect(
        required("FVOCI_LIBSQL_URL")?,
        required("FVOCI_LIBSQL_AUTH_TOKEN")?,
        1,
    )
    .await
    .map_err(|_| "TURSO_UI_CONNECT_FAILED")?;
    let backend = Backend::LibsqlRemote(remote);
    let mut native_outcome =
        json!({"operation":"failed","rollback":"not-attempted","commit":"not-attempted"});
    let result = async {
        let value = match mode.as_str() {
            "baseline" => e2e_fixture::capture_baseline(&backend)
                .await
                .map_err(|error| {
                    native_outcome = e2e_fixture::failure_receipt(&error);
                    "TURSO_UI_BASELINE_FAILED"
                })?,
            "observe" => {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(16385)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "TURSO_UI_OBSERVER_INPUT")?;
                if bytes.len() > 16384 {
                    return Err("TURSO_UI_OBSERVER_INPUT");
                }
                let input: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "TURSO_UI_OBSERVER_INPUT")?;
                let workspace = input["workspaceId"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or("TURSO_UI_OBSERVER_INPUT")?;
                let ids = input["documentIds"]
                    .as_array()
                    .ok_or("TURSO_UI_OBSERVER_INPUT")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .and_then(|s| Uuid::parse_str(s).ok())
                            .ok_or("TURSO_UI_OBSERVER_INPUT")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                e2e_fixture::observe_native(&backend, workspace, &ids)
                    .await
                    .map_err(|error| {
                        native_outcome = e2e_fixture::failure_receipt(&error);
                        "TURSO_UI_NATIVE_OBSERVATION_FAILED"
                    })?
            }
            _ => {
                let namespace = required("FVOCI_E2E_TURSO_NAMESPACE")?;
                let keys = Keyring::parse(
                    &required("PASSWORD_PEPPER_KEYS")?,
                    &required("PASSWORD_PEPPER_ACTIVE_KEY_ID")?,
                )
                .map_err(|_| "TURSO_UI_KEYRING_INVALID")?;
                // Same synthetic credentials as the unchanged browser fixtures.
                let owner = mode == "owner";
                let hash = hash_password(if owner { "supersecret1" } else { "memberpass1" }, &keys)
                    .await
                    .map_err(|_| "TURSO_UI_PASSWORD_FAILED")?;
                let input = e2e_fixture::ActorInput {
                    namespace: &namespace,
                    password_hash: &hash,
                    given_name: if owner { "관리자" } else { "협업" },
                    family_name: if owner { "김" } else { "멤버" },
                };
                if owner {
                    e2e_fixture::allocate_owner(&backend, input).await
                } else {
                    e2e_fixture::add_member(&backend, input).await
                }
                .map_err(|error| {
                    native_outcome = e2e_fixture::failure_receipt(&error);
                    "TURSO_UI_ACTOR_FAILED"
                })?
            }
        };
        Ok::<_, &'static str>(value)
    }
    .await;
    let close = backend.close().await;
    let stats = backend.connection_stats();
    let original = result.as_ref().err().copied();
    let drained = close.is_ok() && stats.as_ref().is_ok_and(|s| s.size == 0);
    let mut value = result
        .unwrap_or_else(|code| json!({"originalFailure":code,"nativeOutcome":native_outcome}));
    if original.is_none() {
        value["nativeOutcome"] = json!({"operation":"confirmed","rollback":"confirmed",
            "commit": if matches!(mode.as_str(), "owner" | "member") { "confirmed" } else { "not-attempted" }});
    }
    value["lifecycleDrain"] = json!(if drained { "confirmed" } else { "unconfirmed" });
    value["leases"] = json!(stats.as_ref().ok().map(|s| s.size));
    value["drainOutcome"] = json!(if close.is_ok() { "confirmed" } else { "failed" });
    // SDK does not expose a server Close receipt. Local drain and confirmed
    // transaction finish are reported exactly, without inventing that proof.
    value["serverCloseReceipt"] = json!("not-exposed-by-sdk");
    println!("{value}");
    if let Some(code) = original {
        return Err(code);
    }
    if !drained {
        return Err("TURSO_UI_DRAIN_FAILED");
    }
    Ok(())
}
