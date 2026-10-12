//! Native GNU input measurement for the Task scalar qualification.
//! Measurements are evidence only; product admission uses its sealed profile.

use crate::args::{self, Outcome};
use crate::process;
use crate::rust_binaries_archive::file_sha256;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE: &str = "usage: xtask task-scalar-qualification {measure | image-measure --image REF --directory DIRECTORY | {bootstrap,final} --image REF --directory DIRECTORY --target-dir TARGET --sqlite-inputs FILE}";
const READ: Duration = Duration::from_secs(300);

pub fn runtime() -> Result<Value, String> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let (_, kernel_arch) = crate::host::uname().map_err(|e| e.to_string())?;
        if kernel_arch != std::env::consts::ARCH {
            return Err("Task scalar measurement requires the native architecture".into());
        }
        // SAFETY: GNU returns a static NUL-terminated version string.
        let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let archive = Path::new("/usr/lib/locale/locale-archive");
        Ok(json!({
            "schema_version": 1,
            "arch": std::env::consts::ARCH,
            "kernel_arch": kernel_arch,
            "glibc_version": version,
            "locale_archive_sha256": file_sha256(archive).map_err(|e| e.to_string())?,
            "locale_archive_bytes": fs::metadata(archive).map_err(|e| e.to_string())?.len(),
            "locpath_present": std::env::var_os("LOCPATH").is_some(),
        }))
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    Err("Task scalar measurement requires Linux GNU".into())
}

fn docker(arguments: &[OsString]) -> Result<Vec<u8>, String> {
    let done = process::run_owned(Command::new("docker").args(arguments), READ)
        .map_err(|e| e.to_string())?;
    if !done.status.success() {
        return Err(format!(
            "docker measurement failed: {}: {}",
            done.status,
            String::from_utf8_lossy(&[done.stdout, done.stderr].concat())
        ));
    }
    Ok(done.stdout)
}

fn docker_text(arguments: &[&str]) -> Result<String, String> {
    let arguments: Vec<_> = arguments.iter().map(OsString::from).collect();
    String::from_utf8(docker(&arguments)?)
        .map(|s| s.trim().to_owned())
        .map_err(|e| e.to_string())
}

fn run_container(
    image: &str,
    entrypoint: &str,
    mounts: &[String],
    environment: &[String],
    argv: &[&str],
) -> Result<(String, String), String> {
    let expected = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => return Err("Task scalar image architecture unsupported".into()),
    };
    let arch = docker_text(&["image", "inspect", "--format", "{{.Architecture}}", image])?;
    if arch != expected {
        return Err("Task scalar image measurement requires the native architecture".into());
    }
    let image_id = docker_text(&["image", "inspect", "--format", "{{.Id}}", image])?;
    // SAFETY: these calls have no arguments or borrowed memory.
    let user = unsafe { format!("{}:{}", libc::geteuid(), libc::getegid()) };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = format!("fvoci-task-scalar-{}-{nonce}", std::process::id());
    let owner = format!("io.fvoci.task-scalar-qualification.owner={name}");
    let mut arguments: Vec<OsString> = [
        "create",
        "--name",
        &name,
        "--label",
        &owner,
        "--network",
        "none",
        "--read-only",
        "--user",
        &user,
        "--workdir",
        "/opt/fvoci",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    for mount in mounts {
        arguments.extend([OsString::from("--mount"), OsString::from(mount)]);
    }
    for value in environment {
        arguments.extend([OsString::from("--env"), OsString::from(value)]);
    }
    arguments.extend([
        OsString::from("--entrypoint"),
        OsString::from(entrypoint),
        OsString::from(&image_id),
    ]);
    arguments.extend(argv.iter().map(OsString::from));
    let created = docker(&arguments).and_then(|bytes| {
        String::from_utf8(bytes)
            .map(|s| s.trim().to_owned())
            .map_err(|e| e.to_string())
    });
    let container = match created {
        Ok(id) => id,
        Err(error) => {
            // create may have reached the daemon before its client failed.
            // Reclaim by the preselected name only after matching our label.
            let owned = docker_text(&[
                "inspect",
                "--format",
                "{{index .Config.Labels \"io.fvoci.task-scalar-qualification.owner\"}}",
                &name,
            ]);
            if owned.as_deref() == Ok(name.as_str()) {
                docker_text(&["rm", "--force", &name])
                    .map_err(|cleanup| format!("{error}; cleanup: {cleanup}"))?;
            } else {
                return Err(format!(
                    "{error}; create cleanup could not verify an owned container"
                ));
            }
            return Err(error);
        }
    };
    let measured = (|| {
        let raw = docker_text(&["start", "--attach", &container])?;
        let state = docker_text(&[
            "inspect",
            "--format",
            "{{.State.Status}} {{.State.ExitCode}}",
            &container,
        ])?;
        if state != "exited 0" {
            return Err(format!(
                "Task scalar measurement container did not exit successfully: {state}"
            ));
        }
        Ok(raw)
    })();
    // A timeout or failed start must also remove only this owned container.
    let cleanup = docker_text(&["rm", "--force", &container]);
    match (measured, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(output), Ok(_)) => Ok((image_id, output)),
    }
}

fn image_measure(image: &str, directory: &Path) -> Result<(), String> {
    let host = runtime()?;
    fs::create_dir(directory).map_err(|e| e.to_string())?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mount = format!(
        "type=bind,src={},dst=/qualification-xtask,readonly",
        executable.display()
    );
    let (image_id, raw) = run_container(
        image,
        "/qualification-xtask",
        &[mount],
        &[],
        &["task-scalar-qualification", "measure"],
    )?;
    let product: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if product["arch"] != host["arch"] {
        return Err("Task scalar container architecture differs".into());
    }
    let value = json!({
        "schema_version": 1, "image_id": image_id, "host": host,
        "product": product, "container_exited": true, "container_removed": true,
    });
    fs::write(directory.join("runtime.json"), pretty(&value)).map_err(|e| e.to_string())?;
    print!("{}", pretty(&value));
    Ok(())
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap() + "\n"
}

const BOOTSTRAP: &[&str] = &[
    "db::tasks::selected_task_projection_tests::selected_decimal_orders_exact_large_and_fractional_values",
    "db::tasks::selected_task_projection_tests::selected_query_fraction_matches_pg7_text_cast_vectors",
    "db::tasks::selected_task_projection_tests::selected_top_k_nulls_last_both_directions_and_uuid_ties",
    "db::tasks::selected_task_projection_tests::selected_tzif_preserves_year_9999_dst_and_due_date_precedence",
    "db::tasks::selected_task_projection_tests::selected_window_uses_utc_endpoints_and_literal_title_characters",
    "db::tasks::selected_task_projection_tests::selected_title_byte_order_and_cursor_changed_value_refusal",
    "db::tasks::selected_task_projection_tests::selected_gnu_literal_lower_matches_original_pg_profile",
    "db::tasks::selected_task_projection_tests::selected_gnu_default_and_c_sort_use_original_pg_orders",
    "db::tasks::selected_task_projection_tests::selected_gnu_wrong_plan_and_nul_preserve_selection",
    "db::tasks::selected_task_projection_tests::selected_scalar_profile_rejects_wrong_catalog_hash_schema_and_unqualified_runtime",
    "db::tasks::selected_task_projection_tests::selected_scalar_full_catalog_uses_reference_data_and_exact_case_fallback",
    "db::tasks::selected_project_list_tests::selected_task_list_reference_qualification_gnu_text_pages_literal_filter_and_error_recovery",
];
const FINAL_LIBRARY: &[&str] = &[
    "db::tasks::selected_project_list_tests::selected_scalar_qualified_runtime_admission_and_exact_drift_refusal",
    "db::tasks::selected_project_list_tests::selected_scalar_qualified_runtime_locpath_refusal_in_serial_child",
    "db::tasks::selected_project_list_tests::selected_task_list_gnu_text_pages_literal_filter_and_error_recovery",
    "db::tasks::selected_project_list_tests::selected_task_list_exhaustive_batches_exact_decimal_count_cursor_and_hydration",
    "db::tasks::selected_project_list_tests::selected_task_list_current_authority_refusals_restore_healthy_progress",
    "db::tasks::selected_project_list_tests::selected_task_list_catalog_reference_and_current_schema_cursor_refusals",
    "db::labels::selected_project_read_tests::selected_labels_read_nonempty_order_current_grants_and_credential_refusals",
];
const HTTP: &str = "sqlite_task_port::sqlite_task_create_http_readback_and_refusals";

fn captured(program: &str, argv: &[&str]) -> Result<String, String> {
    let done =
        process::run_owned(Command::new(program).args(argv), READ).map_err(|e| e.to_string())?;
    if !done.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&done.stderr)
        ));
    }
    String::from_utf8(done.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| e.to_string())
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::host::hex(&Sha256::digest(bytes))
}

fn aarch64_elf(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Task scalar executable must be a physical file".into());
    }
    let mut header = [0; 20];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .read_exact(&mut header)
        .map_err(|e| e.to_string())?;
    // ELF64, little endian, EM_AARCH64; never qualify a foreign ELF via binfmt.
    if &header[..6] != b"\x7fELF\x02\x01" || u16::from_le_bytes([header[18], header[19]]) != 183 {
        return Err("Task scalar qualification executable is not native AArch64 ELF".into());
    }
    Ok(())
}

fn dependency_identity(image: &str, executable: &Path) -> Result<Vec<String>, String> {
    let mount = format!(
        "type=bind,src={},dst=/qualification-test,readonly",
        executable.display()
    );
    let (_, output) = run_container(
        image,
        "/usr/bin/ldd",
        &[mount],
        &["LC_ALL=C".into()],
        &["/qualification-test"],
    )?;
    Ok(output
        .lines()
        .map(|line| {
            line.split_whitespace()
                .filter(|part| !part.starts_with("(0x"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect())
}

fn build_environment_identity() -> String {
    let environment: std::collections::BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            key.starts_with("CARGO_PROFILE_")
                || key.starts_with("CARGO_TARGET_")
                || key.starts_with("SQLITE3_")
                || matches!(
                    key.as_ref(),
                    "RUSTFLAGS"
                        | "CARGO_ENCODED_RUSTFLAGS"
                        | "RUSTC"
                        | "RUSTC_WRAPPER"
                        | "RUSTC_WORKSPACE_WRAPPER"
                        | "CARGO_BUILD_TARGET"
                        | "CARGO_INCREMENTAL"
                        | "LIBCLANG_PATH"
                        | "CC"
                        | "CFLAGS"
                        | "AR"
                )
        })
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    sha(serde_json::to_string(&environment).unwrap().as_bytes())
}

/// Hash the actual production scalar region, excluding only the checksum of
/// the admission envelope, which necessarily changes after a receipt is sealed.
fn consumer_identity(source: &str, profile_sha256: &str) -> Result<String, String> {
    let start = source
        .find("enum TaskScalarUse {")
        .ok_or("scalar consumer start missing")?;
    let end = source
        .find("/// Source `listTasks(projectId = null)`")
        .ok_or("scalar consumer end missing")?;
    let consumer = source
        .get(start..end)
        .ok_or("scalar consumer boundaries invalid")?;
    if consumer.matches(profile_sha256).count() != 1 {
        return Err("scalar consumer must contain exactly its sealed profile checksum".into());
    }
    Ok(sha(consumer
        .replace(profile_sha256, "<sealed-profile-checksum>")
        .as_bytes()))
}

fn native_inputs(sdk: &Path) -> Result<Value, String> {
    let mut files = serde_json::Map::new();
    for (key, variable, member) in [
        ("sqlite_static", "SQLITE3_LIB_DIR", "libsqlite3.a"),
        ("sqlite_header", "SQLITE3_INCLUDE_DIR", "sqlite3.h"),
    ] {
        let directory = std::env::var_os(variable).ok_or_else(|| format!("{variable} missing"))?;
        files.insert(
            key.into(),
            json!(file_sha256(&PathBuf::from(directory).join(member)).map_err(|e| e.to_string())?),
        );
    }
    files.insert(
        "prepared_sdk".into(),
        json!(file_sha256(sdk).map_err(|e| e.to_string())?),
    );
    Ok(Value::Object(files))
}

fn task_http_executable(target: &str, root: &Path) -> Result<PathBuf, String> {
    let mut command = Command::new("cargo");
    command.args([
        "test",
        "--locked",
        "--offline",
        "--features",
        "db-tests",
        "--test",
        "task_integration",
        "--no-run",
        "--message-format=json",
        "--target-dir",
        target,
    ]);
    crate::selected_library::without_outer_package_env(&mut command);
    let output = command
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Task scalar HTTP build failed".into());
    }
    let records = crate::rust_binaries_cohort::records(
        std::str::from_utf8(&output.stdout).map_err(|e| e.to_string())?,
    )?;
    let expected = crate::rust_binaries_cohort::expectation("task_integration", &[]);
    let entry = crate::rust_binaries_cohort::entry(&records, "task_integration", &expected, root)?;
    Ok(root.join(entry.path))
}

fn qualification(
    image: &str,
    directory: &Path,
    target: &str,
    sdk: &Path,
    final_candidate: bool,
) -> Result<(), String> {
    if std::env::consts::ARCH != "aarch64" {
        return Err("Task scalar qualification requires native aarch64 execution".into());
    }
    let root = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !captured(
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    )?
    .is_empty()
    {
        return Err("Task scalar qualification requires a clean committed source tree".into());
    }
    let image_id = crate::install_image::verify(&root, image, None)?;
    let host = runtime()?;
    let source_sha = captured("git", &["rev-parse", "HEAD"])?;
    if std::env::var("GITHUB_SHA").ok().as_deref() != Some(source_sha.as_str()) {
        return Err("Task scalar qualification checkout SHA differs from its CI producer".into());
    }
    for name in ["GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"] {
        if !std::env::var(name)
            .is_ok_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(format!("{name} must identify the actual CI producer"));
        }
    }
    let native = native_inputs(sdk)?;
    let profile_path = root.join("src/db/task_scalar_pg18_profile.json");
    let profile_sha256 = file_sha256(&profile_path).map_err(|e| e.to_string())?;
    let source = fs::read_to_string(root.join("src/db/tasks.rs")).map_err(|e| e.to_string())?;
    let consumer_sha256 = consumer_identity(&source, &profile_sha256)?;
    fs::create_dir(directory).map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let records = crate::selected_library::build(Some(target))?;
    let (library, _) = crate::selected_library::library_executable(&records)?;
    aarch64_elf(&library)?;
    let http = if final_candidate {
        Some(task_http_executable(
            target,
            &Path::new(target)
                .canonicalize()
                .map_err(|e| e.to_string())?,
        )?)
    } else {
        None
    };
    if let Some(http) = &http {
        aarch64_elf(http)?;
    }
    let mut binaries = json!({"library": file_sha256(&library).map_err(|e| e.to_string())?});
    if let Some(http) = &http {
        binaries["task_http"] = json!(file_sha256(http).map_err(|e| e.to_string())?);
    }
    let mut dependencies = json!({"library": dependency_identity(&image_id, &library)?});
    if let Some(http) = &http {
        dependencies["task_http"] = json!(dependency_identity(&image_id, http)?);
    }
    let inputs = json!({
        "source_sha": source_sha,
        "source_tree": captured("git", &["rev-parse", "HEAD^{tree}"])?,
        "repository": std::env::var("GITHUB_REPOSITORY").map_err(|_| "GITHUB_REPOSITORY missing")?,
        "run_id": std::env::var("GITHUB_RUN_ID").map_err(|_| "GITHUB_RUN_ID missing")?,
        "run_attempt": std::env::var("GITHUB_RUN_ATTEMPT").map_err(|_| "GITHUB_RUN_ATTEMPT missing")?,
        "rustc": captured("rustc", &["-vV"])?,
        "cargo_lock_sha256": file_sha256(&root.join("Cargo.lock")).map_err(|e| e.to_string())?,
        "toolchain_sha256": file_sha256(&root.join("rust-toolchain.toml")).map_err(|e| e.to_string())?,
        "producer_sha256": file_sha256(&std::env::current_exe().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?,
        "build_environment_sha256": build_environment_identity(),
        "profile_sha256": profile_sha256, "consumer_sha256": consumer_sha256,
        "native_inputs": native, "binaries": binaries, "image_id": image_id,
        "dynamic_dependencies": dependencies,
        "library_profile": records.iter().find(|r| r["target"]["kind"] == json!(["lib"])) .map(|r| json!({"features": r["features"], "profile": r["profile"]})),
    });
    fs::write(directory.join("inputs.json"), pretty(&inputs)).map_err(|e| e.to_string())?;
    let temporary = directory.join("tmp");
    fs::create_dir(&temporary).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o711))
        .map_err(|e| e.to_string())?;
    let child = temporary.join("child");
    fs::create_dir(&child).map_err(|e| e.to_string())?;
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let mut evidence = Vec::new();
    let mut data_identity = None;
    let mut selectors = BOOTSTRAP.to_vec();
    if final_candidate {
        selectors.extend_from_slice(FINAL_LIBRARY);
        selectors.push(HTTP);
    }
    crate::selected_library::filters(&(selectors.join("\n") + "\n"))?;
    for (index, selector) in selectors.iter().enumerate() {
        let executable = if *selector == HTTP {
            http.as_ref().unwrap()
        } else {
            &library
        };
        let mounts = [
            format!(
                "type=bind,src={},dst=/qualification-test,readonly",
                executable.display()
            ),
            format!(
                "type=bind,src={},dst=/qualification-source,readonly",
                root.display()
            ),
            format!(
                "type=bind,src={},dst=/qualification-tmp",
                temporary.display()
            ),
        ];
        let mut environment = vec![
            "TMPDIR=/qualification-tmp".into(),
            "CARGO_MANIFEST_DIR=/qualification-source".into(),
        ];
        if selector.ends_with("selected_scalar_qualified_runtime_locpath_refusal_in_serial_child") {
            environment.push("FVOCI_SCALAR_CHILD_RECEIPT_DIR=/qualification-tmp/child".into());
        }
        let (_, output) = run_container(
            &image_id,
            "/qualification-test",
            &mounts,
            &environment,
            &[selector, "--exact", "--test-threads=1", "--show-output"],
        )?;
        fs::write(directory.join(format!("test-{index:02}.log")), &output)
            .map_err(|e| e.to_string())?;
        println!("{output}");
        if let Some(error) = crate::selected_library::result_error(selector, 0, &output) {
            return Err(format!("{error}: {selector}"));
        }
        for line in output.lines() {
            if let Some(identity) = line.strip_prefix("FVOCI_SCALAR_DATA_IDENTITY=") {
                if data_identity.replace(identity.to_owned()).is_some() {
                    return Err("duplicate scalar data identity".into());
                }
            }
        }
        evidence.push(json!({"selector": selector, "argv": [selector, "--exact", "--test-threads=1", "--show-output"], "passed": 1, "ignored": 0, "exit": 0, "output_sha256": sha(output.as_bytes()), "container_exited": true, "container_removed": true}));
    }
    let data_identity =
        data_identity.ok_or("actual scalar data identity missing from bootstrap body")?;
    if data_identity.len() != 64 || !data_identity.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("scalar data identity invalid".into());
    }
    let runner = std::env::current_exe().map_err(|e| e.to_string())?;
    let (_, measured) = run_container(
        &image_id,
        "/qualification-xtask",
        &[format!(
            "type=bind,src={},dst=/qualification-xtask,readonly",
            runner.display()
        )],
        &[],
        &["task-scalar-qualification", "measure"],
    )?;
    let product: Value = serde_json::from_str(&measured).map_err(|e| e.to_string())?;
    if product["arch"] != "aarch64" || product["locpath_present"] != false {
        return Err(
            "Task scalar qualification requires the actual native product GNU input".into(),
        );
    }
    let child_receipt = if final_candidate {
        Some(
            json!({"receipt": serde_json::from_slice::<Value>(&fs::read(child.join("receipt.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?,
            "stdout_sha256": file_sha256(&child.join("stdout")).map_err(|e| e.to_string())?,
            "stderr_sha256": file_sha256(&child.join("stderr")).map_err(|e| e.to_string())?}),
        )
    } else {
        None
    };
    if native_inputs(sdk)? != native
        || file_sha256(&library).map_err(|e| e.to_string())?
            != inputs["binaries"]["library"].as_str().unwrap()
        || http.as_ref().is_some_and(|path| {
            file_sha256(path).ok().as_deref() != inputs["binaries"]["task_http"].as_str()
        })
        || captured("git", &["rev-parse", "HEAD"])? != source_sha
        || !captured(
            "git",
            &["status", "--porcelain", "--untracked-files=normal"],
        )?
        .is_empty()
    {
        return Err("Task scalar source/native inputs changed during qualification".into());
    }
    let receipt = json!({
        "schema_version": 1, "qualification": "aarch64-task-scalar-explicit-selectors-v1",
        "phase": if final_candidate { "final" } else { "bootstrap" },
        "historical_x86_bootstrap14_list": "unavailable; this is a newly explicit selector set",
        "inputs": inputs, "host_runtime": host, "product_runtime": product,
        "data_identity": data_identity, "tests": evidence, "child_receipt": child_receipt,
        "container_exited": true, "container_removed": true,
    });
    let name = if final_candidate {
        "final-receipt.json"
    } else {
        "bootstrap-receipt.json"
    };
    let path = directory.join(name);
    fs::write(&path, pretty(&receipt)).map_err(|e| e.to_string())?;
    println!(
        "{name} sha256={}",
        file_sha256(&path).map_err(|e| e.to_string())?
    );
    Ok(())
}

pub fn main(mut argv: Vec<OsString>) -> i32 {
    let action = if argv.is_empty() {
        OsString::new()
    } else {
        argv.remove(0)
    };
    let specs = match action.to_str() {
        Some("measure") => vec![],
        Some("image-measure") => vec![args::required("image"), args::required("directory")],
        Some("bootstrap" | "final") => vec![
            args::required("image"),
            args::required("directory"),
            args::required("target-dir"),
            args::required("sqlite-inputs"),
        ],
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let parsed = match args::parse(argv, &specs, false) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            println!("{USAGE}");
            return 0;
        }
        Outcome::Usage(error) => {
            eprintln!("{USAGE}\n{error}");
            return 2;
        }
    };
    let result = if action == "measure" {
        runtime().map(|value| print!("{}", pretty(&value)))
    } else if action == "image-measure" {
        image_measure(
            parsed.get("image").unwrap(),
            Path::new(parsed.get("directory").unwrap()),
        )
    } else {
        qualification(
            parsed.get("image").unwrap(),
            Path::new(parsed.get("directory").unwrap()),
            parsed.get("target-dir").unwrap(),
            Path::new(parsed.get("sqlite-inputs").unwrap()),
            action == "final",
        )
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("task-scalar-qualification: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_test_elf_is_accepted_only_when_it_is_native_arm() {
        let result = aarch64_elf(&std::env::current_exe().unwrap());
        if std::env::consts::ARCH == "aarch64" {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().contains("not native AArch64 ELF"));
        }
    }

    #[test]
    fn changing_the_receipt_envelope_keeps_the_consumer_but_runtime_guard_drift_does_not() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let source = fs::read_to_string(root.join("src/db/tasks.rs")).unwrap();
        let checksum = file_sha256(&root.join("src/db/task_scalar_pg18_profile.json")).unwrap();
        let original = consumer_identity(&source, &checksum).unwrap();
        let changed_checksum = "1".repeat(64);
        let changed_envelope = source.replace(&checksum, &changed_checksum);
        assert_eq!(
            consumer_identity(&changed_envelope, &changed_checksum).unwrap(),
            original
        );
        // Mutate only a disposable in-memory copy of the real source.
        let drift = source.replace("if std::env::var_os(\"LOCPATH\").is_some()", "if false");
        assert_ne!(consumer_identity(&drift, &checksum).unwrap(), original);
    }
}
