//! Native GNU input measurement for the Task scalar qualification.
//! Measurements are evidence only; product admission uses its sealed profile.
//! `reference` recaptures the sealed reference bytes from a pinned image and
//! `seal` admits CI-produced bootstrap receipts into the profile envelope.

use crate::args::{self, Outcome};
use crate::process;
use crate::rust_binaries_archive::file_sha256;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE: &str = "usage: xtask task-scalar-qualification {measure | image-measure --image REF --directory DIRECTORY | {bootstrap,final} --image REF --directory DIRECTORY --target-dir TARGET --sqlite-inputs FILE | reference --image NAME:TAG@sha256:DIGEST --directory DIRECTORY | seal --x86-64-receipt FILE --aarch64-receipt FILE}";
const READ: Duration = Duration::from_secs(300);
const PROFILE: &str = "src/db/task_scalar_pg18_profile.json";
const LICENSE: &str = "src/db/task_scalar_pg18_profile.LICENSE";
const CONSUMER: &str = "src/db/tasks.rs";
const OWNER_LABEL: &str = "io.fvoci.task-scalar-qualification.owner";

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

/// Docker's name for the native architecture; foreign images are refused.
fn native_image_arch() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("arm64"),
        "x86_64" => Ok("amd64"),
        _ => Err("Task scalar image architecture unsupported".into()),
    }
}

/// Create (never start) a container under a preselected owned name. A failed
/// create may still have reached the daemon: reclaim by that name only after
/// its label proves ownership.
fn create_owned(options: Vec<OsString>, image_and_argv: Vec<OsString>) -> Result<String, String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = format!("fvoci-task-scalar-{}-{nonce}", std::process::id());
    let owner = format!("{OWNER_LABEL}={name}");
    let mut arguments: Vec<OsString> = ["create", "--name", &name, "--label", &owner]
        .iter()
        .map(OsString::from)
        .collect();
    arguments.extend(options);
    arguments.extend(image_and_argv);
    let created = docker(&arguments).and_then(|bytes| {
        String::from_utf8(bytes)
            .map(|s| s.trim().to_owned())
            .map_err(|e| e.to_string())
    });
    match created {
        Ok(id) => Ok(id),
        Err(error) => {
            let owned = docker_text(&[
                "inspect",
                "--format",
                &format!("{{{{index .Config.Labels \"{OWNER_LABEL}\"}}}}"),
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
            Err(error)
        }
    }
}

fn run_container(
    image: &str,
    entrypoint: &str,
    mounts: &[String],
    environment: &[String],
    argv: &[&str],
) -> Result<(String, String), String> {
    let expected = native_image_arch()?;
    let arch = docker_text(&["image", "inspect", "--format", "{{.Architecture}}", image])?;
    if arch != expected {
        return Err("Task scalar image measurement requires the native architecture".into());
    }
    let image_id = docker_text(&["image", "inspect", "--format", "{{.Id}}", image])?;
    // SAFETY: these calls have no arguments or borrowed memory.
    let user = unsafe { format!("{}:{}", libc::geteuid(), libc::getegid()) };
    let mut options: Vec<OsString> = [
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
        options.extend([OsString::from("--mount"), OsString::from(mount)]);
    }
    for value in environment {
        options.extend([OsString::from("--env"), OsString::from(value)]);
    }
    options.extend([OsString::from("--entrypoint"), OsString::from(entrypoint)]);
    let mut image_and_argv = vec![OsString::from(&image_id)];
    image_and_argv.extend(argv.iter().map(OsString::from));
    let container = create_owned(options, image_and_argv)?;
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

/// ELF e_machine of each architecture the product admits (EM_X86_64,
/// EM_AARCH64); any other host has no qualification.
fn elf_machine(arch: &str) -> Option<u16> {
    match arch {
        "x86_64" => Some(62),
        "aarch64" => Some(183),
        _ => None,
    }
}

fn native_machine() -> Result<u16, String> {
    elf_machine(std::env::consts::ARCH).ok_or_else(|| {
        "Task scalar qualification requires native x86_64 or aarch64 execution".into()
    })
}

fn native_elf(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let machine = native_machine()?;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Task scalar executable must be a physical file".into());
    }
    let mut header = [0; 20];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .read_exact(&mut header)
        .map_err(|e| e.to_string())?;
    // ELF64, little endian, native e_machine; never qualify a foreign ELF via binfmt.
    if &header[..6] != b"\x7fELF\x02\x01" || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err(format!(
            "Task scalar qualification executable is not native {} ELF",
            std::env::consts::ARCH
        ));
    }
    Ok(())
}

fn qualification_name(arch: &str) -> String {
    format!("{arch}-task-scalar-explicit-selectors-v1")
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
    native_machine()?;
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
    let profile_path = root.join(PROFILE);
    let profile_sha256 = file_sha256(&profile_path).map_err(|e| e.to_string())?;
    let source = fs::read_to_string(root.join(CONSUMER)).map_err(|e| e.to_string())?;
    let consumer_sha256 = consumer_identity(&source, &profile_sha256)?;
    fs::create_dir(directory).map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let records = crate::selected_library::build(Some(target))?;
    let (library, _) = crate::selected_library::library_executable(&records)?;
    native_elf(&library)?;
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
        native_elf(http)?;
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
    if product["arch"] != std::env::consts::ARCH || product["locpath_present"] != false {
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
        "schema_version": 1, "qualification": qualification_name(std::env::consts::ARCH),
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

/// Product field order of `TaskScalarReference` and `TaskScalarZone`
/// (`src/db/tasks.rs`); the sealed file itself has sorted keys.
const REFERENCE_FIELDS: [&str; 9] = [
    "encoding",
    "provider",
    "deterministic",
    "lc_collate",
    "lc_ctype",
    "collation_version",
    "tzdata_version",
    "image_digest",
    "names_sha256",
];
const ZONE_FIELDS: [&str; 3] = ["name", "tzif_base64", "sha256"];

/// The product's data identity: SHA-256 of serde's compact encoding of
/// `(schema_version, reference, zones)` in product field order. The runtime
/// envelope that later carries receipts is not part of it.
fn data_identity(profile: &Value) -> Result<String, String> {
    let invalid = || "Task scalar profile shape differs from the product decoder".to_string();
    let reference = profile["reference"].as_object().ok_or_else(invalid)?;
    let zones = profile["zones"].as_array().ok_or_else(invalid)?;
    if reference.len() != REFERENCE_FIELDS.len() {
        return Err(invalid());
    }
    let schema = profile["schema_version"].as_u64().ok_or_else(invalid)?;
    let mut encoded = format!("[{schema},{{");
    for (index, field) in REFERENCE_FIELDS.iter().enumerate() {
        let value = reference.get(*field).ok_or_else(invalid)?;
        if *field == "deterministic" && !value.is_boolean()
            || *field != "deterministic" && !value.is_string()
        {
            return Err(invalid());
        }
        let separator = if index == 0 { "" } else { "," };
        encoded.push_str(&format!("{separator}\"{field}\":{value}"));
    }
    encoded.push_str("},[");
    for (index, zone) in zones.iter().enumerate() {
        let zone = zone
            .as_object()
            .filter(|zone| zone.len() == ZONE_FIELDS.len())
            .ok_or_else(invalid)?;
        encoded.push_str(if index == 0 { "{" } else { ",{" });
        for (position, field) in ZONE_FIELDS.iter().enumerate() {
            let value = zone
                .get(*field)
                .filter(|value| value.is_string())
                .ok_or_else(invalid)?;
            let separator = if position == 0 { "" } else { "," };
            encoded.push_str(&format!("{separator}\"{field}\":{value}"));
        }
        encoded.push('}');
    }
    encoded.push_str("]]");
    Ok(sha(encoded.as_bytes()))
}

/// The sealed file's own encoding: sorted keys, compact, final newline.
fn canonical(value: &Value) -> String {
    value.to_string() + "\n"
}

/// A lowercase SHA-256 hex digest, nonzero as the product decoder requires.
fn digest_text(text: Option<&str>) -> bool {
    text.is_some_and(|text| {
        text.len() == 64
            && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            && text.bytes().any(|b| b != b'0')
    })
}

enum ImageNode {
    File(Vec<u8>),
    Symlink(String),
    /// Absolute image path of the hard-linked file.
    Hardlink(String),
}

/// Add the entries of a `docker cp CONTAINER:SOURCE -` archive under their
/// absolute image paths.
fn image_nodes(
    archive: &[u8],
    source: &str,
    nodes: &mut BTreeMap<String, ImageNode>,
) -> Result<(), String> {
    use std::io::Read;
    let parent = source
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|_| source.starts_with('/'))
        .ok_or("image path must be absolute")?;
    let absolute = |path: &Path| -> Result<String, String> {
        let text = path.to_str().ok_or("image path is not UTF-8")?;
        normalize(&format!("{parent}/{}", text.trim_end_matches('/')))
    };
    let mut archive = tar::Archive::new(archive);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = absolute(&entry.path().map_err(|e| e.to_string())?)?;
        let kind = entry.header().entry_type();
        let link = entry
            .link_name()
            .map_err(|e| e.to_string())?
            .map(|link| link.to_str().map(str::to_owned));
        let link = || {
            link.clone()
                .flatten()
                .ok_or_else(|| format!("{path}: link target missing"))
        };
        let node = if kind.is_dir() {
            continue;
        } else if kind.is_file() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            ImageNode::File(bytes)
        } else if kind.is_symlink() {
            ImageNode::Symlink(link()?)
        } else if kind.is_hard_link() {
            ImageNode::Hardlink(absolute(Path::new(&link()?))?)
        } else {
            return Err(format!("{path}: unsupported image entry type"));
        };
        if nodes.insert(path.clone(), node).is_some() {
            return Err(format!("{path}: duplicate image entry"));
        }
    }
    Ok(())
}

fn normalize(path: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("image path escapes its root")?;
            }
            part => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

struct Resolved<'a> {
    path: String,
    /// (link path, link target) hops in the order the image resolves them.
    chain: Vec<(String, String)>,
    bytes: &'a [u8],
}

/// Resolve `path` inside the image root, never on the host.
fn resolve<'a>(nodes: &'a BTreeMap<String, ImageNode>, path: &str) -> Result<Resolved<'a>, String> {
    let mut current = normalize(path)?;
    let mut chain = Vec::new();
    for _ in 0..16 {
        // A linked directory above the entry is outside the original layout.
        let mut ancestor = current.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/').filter(|(p, _)| !p.is_empty()) {
            if matches!(nodes.get(parent), Some(ImageNode::Symlink(_))) {
                return Err(format!("{current}: linked directory {parent} unsupported"));
            }
            ancestor = parent;
        }
        match nodes.get(&current) {
            Some(ImageNode::File(bytes)) => {
                return Ok(Resolved {
                    path: current,
                    chain,
                    bytes,
                })
            }
            Some(ImageNode::Hardlink(target)) => match nodes.get(target) {
                Some(ImageNode::File(bytes)) => {
                    return Ok(Resolved {
                        path: current,
                        chain,
                        bytes,
                    })
                }
                _ => return Err(format!("{current}: hard link target is not a file")),
            },
            Some(ImageNode::Symlink(target)) => {
                let next = if target.starts_with('/') {
                    target.clone()
                } else {
                    let (parent, _) = current.rsplit_once('/').unwrap_or_default();
                    format!("{parent}/{target}")
                };
                chain.push((current, target.clone()));
                current = normalize(&next)?;
            }
            None => return Err(format!("{current} is absent from the reference image")),
        }
    }
    Err(format!("{path}: link chain too long"))
}

/// The PROVENANCE table's alias column: the original JSON spelling.
fn chain_text(chain: &[(String, String)]) -> String {
    let hops: Vec<String> = chain
        .iter()
        .map(|(path, target)| {
            format!(
                "{{\"path\": {}, \"target\": {}, \"type\": \"symlink\"}}",
                json!(path),
                json!(target)
            )
        })
        .collect();
    format!("[{}]", hops.join(", "))
}

/// Reference file identities read through `docker cp -L`.
const IDENTITY_FILES: [&str; 5] = [
    "/etc/os-release",
    "/usr/share/doc/tzdata/copyright",
    "/usr/share/zoneinfo/tzdata.zi",
    "/var/lib/dpkg/info/tzdata.md5sums",
    "/var/lib/dpkg/status",
];

fn package_version(status: &str, package: &str) -> Option<String> {
    let header = format!("Package: {package}");
    status
        .split("\n\n")
        .find(|stanza| stanza.lines().any(|line| line == header))?
        .lines()
        .find_map(|line| line.strip_prefix("Version: "))
        .map(str::to_owned)
}

/// Recapture the sealed zone names' TZif bytes and tzdata identity from a
/// never-started container of a digest-pinned reference image.
fn reference(image: &str, directory: &Path) -> Result<(), String> {
    use base64::Engine;
    let (name, digest) = image
        .split_once('@')
        .filter(|(name, digest)| name.contains(':') && digest_text(digest.strip_prefix("sha256:")))
        .ok_or("reference image must be NAME:TAG@sha256:DIGEST")?;
    let repository = name
        .rsplit_once(':')
        .map(|(repository, _)| repository)
        .unwrap();
    let root = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let sealed_bytes = fs::read(root.join(PROFILE)).map_err(|e| e.to_string())?;
    let sealed: Value = serde_json::from_slice(&sealed_bytes).map_err(|e| e.to_string())?;
    if canonical(&sealed).as_bytes() != sealed_bytes {
        return Err("sealed profile is not in its canonical encoding".into());
    }
    let names: Vec<&str> = sealed["zones"]
        .as_array()
        .ok_or("sealed profile has no zones")?
        .iter()
        .map(|zone| zone["name"].as_str().ok_or("sealed zone name invalid"))
        .collect::<Result<_, _>>()?;
    if sha(names.join("\n").as_bytes()) != sealed["reference"]["names_sha256"] {
        return Err("sealed zone names differ from their recorded hash".into());
    }
    let inspected: Value = serde_json::from_str(&docker_text(&[
        "image",
        "inspect",
        "--format",
        "{{json .}}",
        image,
    ])?)
    .map_err(|e| e.to_string())?;
    // The repository digest proves which index the local image came from.
    if !inspected["RepoDigests"]
        .as_array()
        .is_some_and(|digests| digests.contains(&json!(format!("{repository}@{digest}"))))
        || inspected["Os"] != "linux"
        || inspected["Architecture"] != native_image_arch()?
    {
        return Err("reference image is not the pinned native index".into());
    }
    let image_id = inspected["Id"]
        .as_str()
        .ok_or("image ID missing")?
        .to_owned();
    if directory.exists() {
        return Err(format!("{} already exists", directory.display()));
    }
    let container = create_owned(
        ["--pull", "never", "--network", "none"]
            .iter()
            .map(OsString::from)
            .collect(),
        vec![OsString::from(&image_id)],
    )?;
    let copied = (|| -> Result<_, String> {
        let mut nodes = BTreeMap::new();
        for source in ["/usr/share/zoneinfo", "/etc/localtime"] {
            let archive =
                docker(&["cp", format!("{container}:{source}").as_str(), "-"].map(OsString::from))?;
            image_nodes(&archive, source, &mut nodes)?;
        }
        let mut files = BTreeMap::new();
        for source in IDENTITY_FILES {
            let archive = docker(
                &["cp", "-L", format!("{container}:{source}").as_str(), "-"].map(OsString::from),
            )?;
            let mut followed = BTreeMap::new();
            image_nodes(&archive, source, &mut followed)?;
            match followed.remove(source) {
                Some(ImageNode::File(bytes)) if followed.is_empty() => {
                    files.insert(source, bytes);
                }
                _ => return Err(format!("{source} is not one regular file")),
            }
        }
        Ok((nodes, files))
    })();
    // The never-started container is removed on every path.
    let cleanup = docker_text(&["rm", "--force", &container]);
    let (nodes, files) = match (copied, cleanup) {
        (Err(error), Err(cleanup)) => return Err(format!("{error}; cleanup: {cleanup}")),
        (Err(error), _) | (_, Err(error)) => return Err(error),
        (Ok(copied), Ok(_)) => copied,
    };
    let license = fs::read(root.join(LICENSE)).map_err(|e| e.to_string())?;
    if files["/usr/share/doc/tzdata/copyright"] != license {
        return Err("tzdata copyright differs from the retained LICENSE bytes".into());
    }
    let zi =
        std::str::from_utf8(&files["/usr/share/zoneinfo/tzdata.zi"]).map_err(|e| e.to_string())?;
    let tzdata_version = zi
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# version "))
        .filter(|version| !version.is_empty() && version.bytes().all(|b| b.is_ascii_alphanumeric()))
        .ok_or("tzdata.zi version line missing")?;
    // The zone files must belong to the installed tzdata package version.
    let status = String::from_utf8_lossy(&files["/var/lib/dpkg/status"]);
    if !package_version(&status, "tzdata")
        .is_some_and(|version| version.starts_with(&format!("{tzdata_version}-")))
    {
        return Err("tzdata.zi version differs from the installed tzdata package".into());
    }
    let mut zones = Vec::new();
    let mut rows = String::new();
    let mut changed = Vec::new();
    for (name, sealed_zone) in names.iter().zip(sealed["zones"].as_array().unwrap()) {
        let resolved = resolve(&nodes, &format!("/usr/share/zoneinfo/{name}"))?;
        if !resolved.bytes.starts_with(b"TZif") {
            return Err(format!("{name} is not TZif"));
        }
        let checksum = sha(resolved.bytes);
        if sealed_zone["sha256"] != checksum.as_str() {
            changed
                .push(json!({"name": name, "sealed": sealed_zone["sha256"], "captured": checksum}));
        }
        rows.push_str(&format!(
            "| {name} | {} | {checksum} | {} | {} |\n",
            resolved.path,
            resolved.bytes.len(),
            chain_text(&resolved.chain)
        ));
        zones.push(json!({
            "name": name,
            "tzif_base64": base64::engine::general_purpose::STANDARD.encode(resolved.bytes),
            "sha256": checksum,
        }));
    }
    let mut profile = sealed.clone();
    profile["reference"]["tzdata_version"] = json!(tzdata_version);
    profile["reference"]["image_digest"] = json!(digest);
    profile["runtime_profiles"] = json!([]);
    profile["zones"] = Value::Array(zones);
    let encoded = canonical(&profile);
    let identities: Vec<String> = files
        .iter()
        .map(|(path, bytes)| {
            format!(
                "{}: {{\"bytes\": {}, \"sha256\": \"{}\"}}",
                json!(path),
                bytes.len(),
                sha(bytes)
            )
        })
        .collect();
    let os_release = String::from_utf8_lossy(&files["/etc/os-release"]);
    let identity = data_identity(&profile)?;
    let packages = ["tzdata", "libc6", "locales"]
        .map(|package| json!({"package": package, "version": package_version(&status, package)}));
    let provenance = json!({
        "image": image, "image_id": image_id, "repo_digests": inspected["RepoDigests"],
        "platform": {"os": inspected["Os"], "architecture": inspected["Architecture"]},
        "os_release": os_release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=")),
        "packages": packages,
        "tzdata_version": tzdata_version,
        "file_identities": format!("{{{}}}", identities.join(", ")),
        "names_sha256": profile["reference"]["names_sha256"],
        "data_identity": identity,
        "profile_sha256": sha(encoded.as_bytes()),
        "changed_zones": changed,
        "container_removed": true,
    });
    fs::create_dir(directory).map_err(|e| e.to_string())?;
    fs::write(directory.join("profile.json"), &encoded).map_err(|e| e.to_string())?;
    fs::write(directory.join("zones.md"), rows).map_err(|e| e.to_string())?;
    fs::write(directory.join("provenance.json"), pretty(&provenance)).map_err(|e| e.to_string())?;
    print!("{}", pretty(&provenance));
    Ok(())
}

struct Sealed {
    profile: String,
    source: String,
    summary: Value,
}

/// Admit each architecture's CI bootstrap receipt only when it was produced
/// from exactly this checkout, its pending profile and scalar consumer; then
/// derive the runtime envelope and the consumer's checksum from the receipts.
fn seal_profile(
    profile: &[u8],
    source: &str,
    head: &str,
    tree: &str,
    receipts: [(&str, &[u8]); 2],
) -> Result<Sealed, String> {
    let pending: Value = serde_json::from_slice(profile).map_err(|e| e.to_string())?;
    if canonical(&pending).as_bytes() != profile {
        return Err("profile is not in its canonical sealed encoding".into());
    }
    if pending["runtime_profiles"] != json!([]) {
        return Err("seal requires the pending profile without a runtime envelope".into());
    }
    let pending_sha256 = sha(profile);
    if source.matches(&pending_sha256).count() != 1 {
        return Err("the scalar consumer must name the pending profile checksum once".into());
    }
    let consumer = consumer_identity(source, &pending_sha256)?;
    let identity = data_identity(&pending)?;
    let mut runtime_profiles = Vec::new();
    let mut producer = None;
    let mut admitted_receipts = serde_json::Map::new();
    for (arch, bytes) in receipts {
        let refuse = |what: &str| format!("{arch} receipt {what}");
        let receipt: Value = serde_json::from_slice(bytes).map_err(|e| refuse(&e.to_string()))?;
        let inputs = &receipt["inputs"];
        let product = &receipt["product_runtime"];
        if receipt["schema_version"] != 1
            || receipt["qualification"] != qualification_name(arch)
            || receipt["phase"] != "bootstrap"
        {
            return Err(refuse("is not this architecture's bootstrap qualification"));
        }
        if inputs["source_sha"] != head || inputs["source_tree"] != tree {
            return Err(refuse("was not produced from this checkout"));
        }
        if inputs["profile_sha256"] != pending_sha256 || inputs["consumer_sha256"] != consumer {
            return Err(refuse("binds a different profile or scalar consumer"));
        }
        if receipt["data_identity"] != identity {
            return Err(refuse("qualified different reference data"));
        }
        let glibc = product["glibc_version"].as_str().filter(|version| {
            !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        });
        if receipt["host_runtime"]["arch"] != arch
            || product["arch"] != arch
            || product["kernel_arch"] != arch
            || product["locpath_present"] != false
            || glibc.is_none()
            || !digest_text(product["locale_archive_sha256"].as_str())
        {
            return Err(refuse("lacks the native product GNU measurement"));
        }
        if receipt["container_exited"] != true
            || receipt["container_removed"] != true
            || !receipt["child_receipt"].is_null()
        {
            return Err(refuse("lacks container closure"));
        }
        let tests = receipt["tests"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if tests.len() != BOOTSTRAP.len()
            || tests.iter().zip(BOOTSTRAP).any(|(test, selector)| {
                test["selector"] != *selector
                    || test["argv"]
                        != json!([selector, "--exact", "--test-threads=1", "--show-output"])
                    || test["passed"] != 1
                    || test["ignored"] != 0
                    || test["exit"] != 0
                    || test["container_exited"] != true
                    || test["container_removed"] != true
                    || !digest_text(test["output_sha256"].as_str())
            })
        {
            return Err(refuse("does not show each bootstrap body passing once"));
        }
        let run = json!({
            "repository": inputs["repository"],
            "run_id": inputs["run_id"],
            "run_attempt": inputs["run_attempt"],
        });
        let digits = |key: &str| {
            run[key]
                .as_str()
                .is_some_and(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        };
        if !run["repository"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
            || !digits("run_id")
            || !digits("run_attempt")
        {
            return Err(refuse("does not identify its CI producer"));
        }
        if *producer.get_or_insert_with(|| run.clone()) != run {
            return Err("receipts come from different CI runs".into());
        }
        let runtime = json!({
            "arch": arch,
            "glibc_version": glibc,
            "locale_archive_sha256": product["locale_archive_sha256"],
            "qualification_sha256": sha(bytes),
        });
        admitted_receipts.insert(arch.to_owned(), runtime.clone());
        runtime_profiles.push(runtime);
    }
    let mut admitted = pending;
    admitted["runtime_profiles"] = Value::Array(runtime_profiles);
    let encoded = canonical(&admitted);
    let admitted_sha256 = sha(encoded.as_bytes());
    let updated = source.replacen(&pending_sha256, &admitted_sha256, 1);
    if consumer_identity(&updated, &admitted_sha256)? != consumer {
        return Err("sealing changed the scalar consumer".into());
    }
    Ok(Sealed {
        profile: encoded,
        source: updated,
        summary: json!({
            "producer": producer, "source_sha": head, "data_identity": identity,
            "consumer_sha256": consumer, "pending_profile_sha256": pending_sha256,
            "profile_sha256": admitted_sha256, "runtime_profiles": admitted_receipts,
        }),
    })
}

fn seal(x86_64: &Path, aarch64: &Path) -> Result<(), String> {
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
        return Err("seal requires a clean committed source tree".into());
    }
    let head = captured("git", &["rev-parse", "HEAD"])?;
    let tree = captured("git", &["rev-parse", "HEAD^{tree}"])?;
    let profile = fs::read(root.join(PROFILE)).map_err(|e| e.to_string())?;
    let source = fs::read_to_string(root.join(CONSUMER)).map_err(|e| e.to_string())?;
    let x86_64 = fs::read(x86_64).map_err(|e| e.to_string())?;
    let aarch64 = fs::read(aarch64).map_err(|e| e.to_string())?;
    let sealed = seal_profile(
        &profile,
        &source,
        &head,
        &tree,
        [("x86_64", &x86_64), ("aarch64", &aarch64)],
    )?;
    fs::write(root.join(PROFILE), &sealed.profile).map_err(|e| e.to_string())?;
    fs::write(root.join(CONSUMER), &sealed.source).map_err(|e| e.to_string())?;
    print!("{}", pretty(&sealed.summary));
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
        Some("reference") => vec![args::required("image"), args::required("directory")],
        Some("seal") => vec![
            args::required("x86-64-receipt"),
            args::required("aarch64-receipt"),
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
    } else if action == "reference" {
        reference(
            parsed.get("image").unwrap(),
            Path::new(parsed.get("directory").unwrap()),
        )
    } else if action == "seal" {
        seal(
            Path::new(parsed.get("x86-64-receipt").unwrap()),
            Path::new(parsed.get("aarch64-receipt").unwrap()),
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

    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
    }

    #[test]
    fn actual_test_elf_is_accepted_as_native_and_a_foreign_elf_is_refused() {
        native_elf(&std::env::current_exe().unwrap()).unwrap();
        assert_eq!(elf_machine("x86_64"), Some(62));
        assert_eq!(elf_machine("aarch64"), Some(183));
        assert_eq!(elf_machine("riscv64"), None);
        let foreign = match std::env::consts::ARCH {
            "x86_64" => 183u16,
            _ => 62,
        };
        let mut header = b"\x7fELF\x02\x01".to_vec();
        header.resize(18, 0);
        header.extend(foreign.to_le_bytes());
        let directory = std::env::temp_dir().join(format!(
            "fvoci-task-scalar-elf-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("foreign");
        fs::write(&path, &header).unwrap();
        let link = directory.join("link");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
        let refused = native_elf(&path).unwrap_err();
        let linked = native_elf(&link).unwrap_err();
        fs::remove_dir_all(&directory).unwrap();
        assert!(refused.contains("is not native"), "{refused}");
        assert!(linked.contains("physical file"), "{linked}");
        assert_eq!(
            qualification_name(std::env::consts::ARCH),
            format!(
                "{}-task-scalar-explicit-selectors-v1",
                std::env::consts::ARCH
            )
        );
    }

    #[test]
    fn data_identity_and_encoding_mirror_the_product_decoder() {
        let bytes = fs::read(root().join(PROFILE)).unwrap();
        let profile: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(canonical(&profile).as_bytes(), bytes);
        // The product's own sealed expectation, read from its decoder test.
        let source = fs::read_to_string(root().join(CONSUMER)).unwrap();
        let marker = "        assert_eq!(\n            profile.data_identity,\n            \"";
        let start = source.find(marker).unwrap() + marker.len();
        assert_eq!(data_identity(&profile).unwrap(), source[start..start + 64]);
        let mut extra = profile.clone();
        extra["reference"]["unexpected"] = json!("x");
        assert!(data_identity(&extra).is_err());
    }

    enum Fixture<'a> {
        Directory,
        File(&'a [u8]),
        Symlink(&'a str),
        Hardlink(&'a str),
    }

    fn tar_with(entries: &[(&str, Fixture)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, fixture) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(0);
            match fixture {
                Fixture::Directory => {
                    header.set_entry_type(tar::EntryType::Directory);
                    builder.append_data(&mut header, path, &[][..]).unwrap();
                }
                Fixture::File(data) => {
                    header.set_size(data.len() as u64);
                    header.set_entry_type(tar::EntryType::Regular);
                    builder.append_data(&mut header, path, *data).unwrap();
                }
                Fixture::Symlink(link) | Fixture::Hardlink(link) => {
                    header.set_entry_type(if matches!(fixture, Fixture::Hardlink(_)) {
                        tar::EntryType::Link
                    } else {
                        tar::EntryType::Symlink
                    });
                    builder.append_link(&mut header, path, link).unwrap();
                }
            }
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn reference_links_resolve_inside_the_image_like_the_original_table() {
        let utc: &[u8] = b"TZif-utc";
        let zoneinfo = tar_with(&[
            ("zoneinfo", Fixture::Directory),
            ("zoneinfo/Etc/UTC", Fixture::File(utc)),
            ("zoneinfo/Pacific/Honolulu", Fixture::File(b"TZif-hst")),
            ("zoneinfo/Pacific/Johnston", Fixture::Symlink("Honolulu")),
            ("zoneinfo/UTC", Fixture::Symlink("Etc/UTC")),
            ("zoneinfo/localtime", Fixture::Symlink("/etc/localtime")),
            ("zoneinfo/Zulu", Fixture::Hardlink("zoneinfo/Etc/UTC")),
            ("zoneinfo/Loop", Fixture::Symlink("Loop")),
            ("zoneinfo/Escape", Fixture::Symlink("../../../../x")),
            ("zoneinfo/posix", Fixture::Symlink(".")),
        ]);
        let etc = tar_with(&[("localtime", Fixture::Symlink("/usr/share/zoneinfo/Etc/UTC"))]);
        let mut nodes = BTreeMap::new();
        image_nodes(&zoneinfo, "/usr/share/zoneinfo", &mut nodes).unwrap();
        image_nodes(&etc, "/etc/localtime", &mut nodes).unwrap();
        let localtime = resolve(&nodes, "/usr/share/zoneinfo/localtime").unwrap();
        assert_eq!(localtime.path, "/usr/share/zoneinfo/Etc/UTC");
        assert_eq!(localtime.bytes, utc);
        assert_eq!(
            chain_text(&localtime.chain),
            "[{\"path\": \"/usr/share/zoneinfo/localtime\", \"target\": \"/etc/localtime\", \"type\": \"symlink\"}, {\"path\": \"/etc/localtime\", \"target\": \"/usr/share/zoneinfo/Etc/UTC\", \"type\": \"symlink\"}]"
        );
        let johnston = resolve(&nodes, "/usr/share/zoneinfo/Pacific/Johnston").unwrap();
        assert_eq!(johnston.path, "/usr/share/zoneinfo/Pacific/Honolulu");
        assert_eq!(
            chain_text(&johnston.chain),
            "[{\"path\": \"/usr/share/zoneinfo/Pacific/Johnston\", \"target\": \"Honolulu\", \"type\": \"symlink\"}]"
        );
        let zulu = resolve(&nodes, "/usr/share/zoneinfo/Zulu").unwrap();
        assert_eq!(
            (zulu.path.as_str(), zulu.bytes),
            ("/usr/share/zoneinfo/Zulu", utc)
        );
        assert_eq!(chain_text(&zulu.chain), "[]");
        for refused in ["Loop", "Escape", "posix/UTC", "Mars/Olympus"] {
            assert!(resolve(&nodes, &format!("/usr/share/zoneinfo/{refused}")).is_err());
        }
        let mut duplicate = BTreeMap::new();
        image_nodes(&etc, "/etc/localtime", &mut duplicate).unwrap();
        assert!(image_nodes(&etc, "/etc/localtime", &mut duplicate).is_err());
        assert_eq!(
            package_version(
                "Package: tzdata\nVersion: 2026c-0+deb13u1\n\nPackage: x\n",
                "tzdata"
            )
            .as_deref(),
            Some("2026c-0+deb13u1")
        );
    }

    struct SealCase {
        profile: Vec<u8>,
        source: String,
        receipts: [Value; 2],
    }

    /// A pending profile and its consumer derived from the real sealed input;
    /// only the runtime envelope and its checksum differ.
    fn seal_case() -> SealCase {
        let mut profile: Value =
            serde_json::from_slice(&fs::read(root().join(PROFILE)).unwrap()).unwrap();
        profile["runtime_profiles"] = json!([]);
        let pending = canonical(&profile).into_bytes();
        let source = fs::read_to_string(root().join(CONSUMER)).unwrap();
        let sealed = file_sha256(&root().join(PROFILE)).unwrap();
        let source = source.replacen(&sealed, &sha(&pending), 1);
        let consumer = consumer_identity(&source, &sha(&pending)).unwrap();
        let identity = data_identity(&profile).unwrap();
        let receipts = ["x86_64", "aarch64"].map(|arch| {
            let tests: Vec<Value> = BOOTSTRAP.iter().map(|selector| json!({
                "selector": selector, "argv": [selector, "--exact", "--test-threads=1", "--show-output"],
                "passed": 1, "ignored": 0, "exit": 0, "output_sha256": "a".repeat(64),
                "container_exited": true, "container_removed": true,
            })).collect();
            json!({
                "schema_version": 1, "qualification": qualification_name(arch), "phase": "bootstrap",
                "inputs": {"source_sha": "1".repeat(40), "source_tree": "2".repeat(40),
                    "repository": "AISFlow/fvoci", "run_id": "42", "run_attempt": "1",
                    "profile_sha256": sha(&pending), "consumer_sha256": consumer},
                "host_runtime": {"arch": arch},
                "product_runtime": {"arch": arch, "kernel_arch": arch, "glibc_version": "2.43",
                    "locale_archive_sha256": "b".repeat(64), "locpath_present": false},
                "data_identity": identity, "tests": tests, "child_receipt": null,
                "container_exited": true, "container_removed": true,
            })
        });
        SealCase {
            profile: pending,
            source,
            receipts,
        }
    }

    fn sealed(case: &SealCase) -> Result<Sealed, String> {
        let receipts = case
            .receipts
            .clone()
            .map(|receipt| pretty(&receipt).into_bytes());
        seal_profile(
            &case.profile,
            &case.source,
            &"1".repeat(40),
            &"2".repeat(40),
            [("x86_64", &receipts[0]), ("aarch64", &receipts[1])],
        )
    }

    #[test]
    fn seal_admits_only_bootstrap_receipts_bound_to_this_checkout() {
        let case = seal_case();
        let admitted = sealed(&case).unwrap();
        let profile: Value = serde_json::from_str(&admitted.profile).unwrap();
        let pending: Value = serde_json::from_slice(&case.profile).unwrap();
        assert_eq!(data_identity(&profile), data_identity(&pending));
        let runtimes = profile["runtime_profiles"].as_array().unwrap();
        for (runtime, (arch, receipt)) in runtimes
            .iter()
            .zip(["x86_64", "aarch64"].iter().zip(&case.receipts))
        {
            assert_eq!(
                runtime,
                &json!({"arch": arch, "glibc_version": "2.43", "locale_archive_sha256": "b".repeat(64),
                    "qualification_sha256": sha(pretty(receipt).as_bytes())})
            );
        }
        let checksum = sha(admitted.profile.as_bytes());
        assert_eq!(admitted.source.matches(&checksum).count(), 1);
        assert_eq!(
            consumer_identity(&admitted.source, &checksum).unwrap(),
            consumer_identity(&case.source, &sha(&case.profile)).unwrap()
        );
        type Mutation = fn(&mut SealCase);
        let refusals: [(&str, Mutation); 15] = [
            ("bootstrap qualification", |c| {
                c.receipts[0]["phase"] = json!("final")
            }),
            ("bootstrap qualification", |c| {
                c.receipts[0]["qualification"] = json!(qualification_name("aarch64"))
            }),
            ("produced from this checkout", |c| {
                c.receipts[1]["inputs"]["source_sha"] = json!("3".repeat(40))
            }),
            ("produced from this checkout", |c| {
                c.receipts[0]["inputs"]["source_tree"] = json!("3".repeat(40))
            }),
            ("different profile or scalar consumer", |c| {
                c.receipts[0]["inputs"]["profile_sha256"] = json!("c".repeat(64))
            }),
            ("different profile or scalar consumer", |c| {
                c.receipts[1]["inputs"]["consumer_sha256"] = json!("c".repeat(64))
            }),
            ("different reference data", |c| {
                c.receipts[0]["data_identity"] = json!("c".repeat(64))
            }),
            ("native product GNU measurement", |c| {
                c.receipts[1]["product_runtime"]["arch"] = json!("x86_64")
            }),
            ("native product GNU measurement", |c| {
                c.receipts[0]["product_runtime"]["locpath_present"] = json!(true)
            }),
            // The product decoder refuses a zero digest; seal must refuse it first.
            ("native product GNU measurement", |c| {
                c.receipts[1]["product_runtime"]["locale_archive_sha256"] = json!("0".repeat(64))
            }),
            ("each bootstrap body", |c| {
                c.receipts[1]["tests"][3]["ignored"] = json!(1)
            }),
            ("each bootstrap body", |c| {
                c.receipts[0]["tests"].as_array_mut().unwrap().pop();
            }),
            ("different CI runs", |c| {
                c.receipts[1]["inputs"]["run_attempt"] = json!("2")
            }),
            ("without a runtime envelope", |c| {
                let mut profile: Value = serde_json::from_slice(&c.profile).unwrap();
                profile["runtime_profiles"] = json!([{"arch": "x86_64"}]);
                c.profile = canonical(&profile).into_bytes();
            }),
            ("canonical sealed encoding", |c| c.profile.insert(1, b' ')),
        ];
        for (name, mutate) in refusals {
            let mut case = seal_case();
            mutate(&mut case);
            let error = sealed(&case).err().unwrap_or_default();
            assert!(error.contains(name), "{name}: {error}");
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
