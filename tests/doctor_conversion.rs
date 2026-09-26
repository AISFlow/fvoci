//! The installed `fvoci-migrate --doctor` must launch its sibling server's
//! internal Markdown child, and fail its named check for broken installations.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

struct Binaries(PathBuf);

impl Binaries {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_BIN_EXE_fvoci-migrate"))
            .parent()
            .unwrap()
            .join(format!("doctor-conversion-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        std::fs::hard_link(
            env!("CARGO_BIN_EXE_fvoci-migrate"),
            path.join("fvoci-migrate"),
        )
        .unwrap();
        Self(path)
    }

    fn server(&self) -> PathBuf {
        self.0.join("fvoci-server")
    }

    fn install_server(&self) {
        std::fs::hard_link(env!("CARGO_BIN_EXE_fvoci-server"), self.server()).unwrap();
    }

    async fn doctor(&self, font_dir: Option<&Path>) -> (i32, Value) {
        let mut cmd = tokio::process::Command::new(self.0.join("fvoci-migrate"));
        cmd.arg("--doctor").env_clear();
        if let Some(font_dir) = font_dir {
            cmd.env("FVOCI_EXPORT_FONT_DIR", font_dir);
        }
        let output = tokio::time::timeout(Duration::from_secs(90), cmd.output())
            .await
            .expect("doctor reached its process deadline")
            .unwrap();
        let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "doctor stdout: {} / stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap_or(-1), report)
    }
}

impl Drop for Binaries {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn converter(report: &Value) -> &Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "document_convert")
        .unwrap_or_else(|| panic!("document_convert check missing: {report}"))
}

#[tokio::test]
async fn migrate_uses_real_sibling_server_for_all_converter_operations() {
    let bins = Binaries::new();
    bins.install_server();
    let (exit, report) = bins.doctor(None).await;
    // Other checks fail because this process deliberately has no DB or config.
    assert_eq!(exit, 1, "{report}");
    assert_eq!(converter(&report)["ok"], true, "{report}");
    assert!(converter(&report).get("detail").is_none(), "{report}");
}

#[tokio::test]
async fn missing_nonexecutable_wrong_server_and_bad_fonts_fail_named_check() {
    let missing = Binaries::new();
    let (exit, report) = missing.doctor(None).await;
    assert_eq!(exit, 1);
    assert_eq!(converter(&report)["ok"], false);
    assert!(converter(&report)["detail"]
        .as_str()
        .unwrap()
        .contains("missing"));

    let nonexecutable = Binaries::new();
    std::fs::write(nonexecutable.server(), b"not an executable").unwrap();
    std::fs::set_permissions(
        nonexecutable.server(),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let (exit, report) = nonexecutable.doctor(None).await;
    assert_eq!(exit, 1);
    assert_eq!(converter(&report)["ok"], false);
    assert!(converter(&report)["detail"]
        .as_str()
        .unwrap()
        .contains("not executable"));

    let wrong = Binaries::new();
    std::fs::copy("/bin/false", wrong.server()).unwrap();
    let (exit, report) = wrong.doctor(None).await;
    assert_eq!(exit, 1);
    assert_eq!(converter(&report)["ok"], false);
    assert!(converter(&report)["detail"]
        .as_str()
        .unwrap()
        .contains("Markdown to Tiptap"));

    let fonts = Binaries::new();
    fonts.install_server();
    let absent = fonts.0.join("absent-fonts");
    let (exit, report) = fonts.doctor(Some(&absent)).await;
    assert_eq!(exit, 1);
    assert_eq!(converter(&report)["ok"], false);
    assert!(converter(&report)["detail"]
        .as_str()
        .unwrap()
        .contains("PDF export"));
    assert!(!absent.exists(), "doctor must not create a font directory");
}
