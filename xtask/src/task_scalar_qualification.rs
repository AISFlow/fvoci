//! Native GNU input measurement for the Task scalar qualification.
//! Measurements are evidence only; product admission uses its sealed profile.

use crate::args::{self, Outcome};
use crate::process;
use crate::rust_binaries_archive::file_sha256;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const USAGE: &str = "usage: xtask task-scalar-qualification {measure | image-measure --image REF --directory DIRECTORY}";
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
            String::from_utf8_lossy(&done.stderr)
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

fn image_measure(image: &str, directory: &Path) -> Result<(), String> {
    let host = runtime()?;
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
    fs::create_dir(directory).map_err(|e| e.to_string())?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mount = format!(
        "type=bind,src={},dst=/qualification-xtask,readonly",
        executable.display()
    );
    // SAFETY: these calls have no arguments or borrowed memory.
    let user = unsafe { format!("{}:{}", libc::geteuid(), libc::getegid()) };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = format!("fvoci-task-scalar-{}-{nonce}", std::process::id());
    let owner = format!("io.fvoci.task-scalar-qualification.owner={name}");
    let created = docker_text(&[
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
        "--mount",
        &mount,
        "--entrypoint",
        "/qualification-xtask",
        &image_id,
        "task-scalar-qualification",
        "measure",
    ]);
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
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if value["arch"] != host["arch"] {
            return Err("Task scalar container architecture differs".into());
        }
        Ok(value)
    })();
    // A timeout or failed start must also remove only this owned container.
    let cleanup = docker_text(&["rm", "--force", &container]);
    match (measured, cleanup) {
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(product), Ok(_)) => {
            let value = json!({
                "schema_version": 1, "image_id": image_id, "host": host,
                "product": product, "container_exited": true, "container_removed": true,
            });
            fs::write(directory.join("runtime.json"), pretty(&value)).map_err(|e| e.to_string())?;
            print!("{}", pretty(&value));
            Ok(())
        }
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap() + "\n"
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
    } else {
        image_measure(
            parsed.get("image").unwrap(),
            Path::new(parsed.get("directory").unwrap()),
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
