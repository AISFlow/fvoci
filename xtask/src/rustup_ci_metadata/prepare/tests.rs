//! Real-filesystem fixtures for the metadata write and its refusals; the
//! Rustup inspection is a fake, so no installed toolchain or compiler runs.

use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

/// Literal Rustup 1.29.1 public `--installed` output, independent of `ROWS`.
const PUBLIC_INSTALLED: &str = "cargo-x86_64-unknown-linux-gnu\n\
                                clippy-x86_64-unknown-linux-gnu\n\
                                rust-std-x86_64-unknown-linux-gnu\n\
                                rustc-x86_64-unknown-linux-gnu";

struct Fake<L, P> {
    list: L,
    point: P,
    calls: usize,
}

impl<L, P> Inspect for Fake<L, P>
where
    L: FnMut(usize) -> Result<String, Refusal>,
    P: FnMut(Point) -> Result<(), Refusal>,
{
    fn component_list(&mut self) -> Result<String, Refusal> {
        self.calls += 1;
        (self.list)(self.calls)
    }
    fn checkpoint(&mut self, point: Point) -> Result<(), Refusal> {
        (self.point)(point)
    }
}

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
    component: PathBuf,
    schema: PathBuf,
    compiler: PathBuf,
    rustup: PathBuf,
    output: PathBuf,
    context: Context,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.root.join("bin"), fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.parent);
    }
}

fn mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let parent = crate::host::mkdtemp("fvoci-rustup-metadata-fixture-", &std::env::temp_dir())
            .unwrap()
            .canonicalize()
            .unwrap();
        let root = parent.join("toolchain");
        fs::create_dir_all(root.join("lib/rustlib")).unwrap();
        let component = root.join(RELATIVE);
        fs::write(&component, canonical()).unwrap();
        mode(&component, 0o644);
        let schema = root.join("lib/rustlib/rust-installer-version");
        fs::write(&schema, b"3\n").unwrap();
        for name in ROWS {
            fs::write(
                root.join("lib/rustlib").join(format!("manifest-{name}")),
                b"file:owned fixture\n",
            )
            .unwrap();
        }
        fs::create_dir(root.join("bin")).unwrap();
        let compiler = root.join("bin/rustc");
        fs::write(&compiler, b"owned synthetic compiled byte canary\n").unwrap();
        mode(&compiler, 0o755);
        let rustup = parent.join("rustup");
        fs::write(&rustup, b"owned synthetic rustup inspection executable\n").unwrap();
        mode(&rustup, 0o755);
        let (rustup_sha256, rustup_identity) = guard::file_facts(&rustup).unwrap();
        let context = Context {
            source_head: "5".repeat(40),
            source_tree: "6".repeat(40),
            job: "collaboration-build".into(),
            rustup_version: "rustup 1.29.1 (d95a37b6a 2026-08-13)".into(),
            rustup_sha256,
            rustup_identity,
        };
        let output = parent.join("receipts");
        Self {
            parent,
            root,
            component,
            schema,
            compiler,
            rustup,
            output,
            context,
        }
    }

    fn run(&self, inspect: &mut dyn Inspect) -> (Result<Map<String, Value>, Refusal>, String) {
        self.run_as(Owner::current(), inspect)
    }

    fn run_as(
        &self,
        owner: Owner,
        inspect: &mut dyn Inspect,
    ) -> (Result<Map<String, Value>, Refusal>, String) {
        let mut out = Vec::new();
        let result = prepare(
            &self.root,
            &self.output,
            &self.rustup,
            &self.context,
            owner,
            inspect,
            &mut out,
        );
        (result, String::from_utf8(out).unwrap())
    }

    fn prepare(&self) -> Result<Map<String, Value>, Refusal> {
        self.run(&mut public()).0
    }

    /// Bytes of the components path when it is a regular file (never opens a FIFO).
    fn regular_contents(&self) -> Option<Vec<u8>> {
        fs::symlink_metadata(&self.component)
            .is_ok_and(|info| info.is_file())
            .then(|| fs::read(&self.component).unwrap())
    }

    fn component_identity(&self) -> Option<Identity> {
        fs::symlink_metadata(&self.component)
            .ok()
            .map(|i| identity(&i))
    }

    /// Refuses with `expected` and leaves the components path untouched.
    fn refuses_with(&self, inspect: &mut dyn Inspect, expected: &str) {
        let before = self.component_identity();
        let raw = self.regular_contents();
        let (result, _) = self.run(inspect);
        assert_eq!(result.unwrap_err().to_string(), expected);
        assert_eq!(self.component_identity(), before);
        assert_eq!(self.regular_contents(), raw);
    }

    fn refuses(&self, expected: &str) {
        self.refuses_with(&mut public(), expected);
    }
}

fn fake<L, P>(list: L, point: P) -> Fake<L, P> {
    Fake {
        list,
        point,
        calls: 0,
    }
}

fn public() -> impl Inspect {
    fake(|_| Ok(PUBLIC_INSTALLED.to_owned()), |_| Ok(()))
}

fn listing(text: &str) -> impl Inspect + '_ {
    fake(move |_| Ok(text.to_owned()), |_| Ok(()))
}

fn rows_raw(rows: &[&str]) -> Vec<u8> {
    let mut raw = rows.join("\n").into_bytes();
    raw.push(b'\n');
    raw
}

fn reversed() -> Vec<u8> {
    let mut rows = ROWS;
    rows.reverse();
    rows_raw(&rows)
}

fn permutations(items: &[&'static str]) -> Vec<Vec<&'static str>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut all = Vec::new();
    for (index, first) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, first);
            all.push(tail);
        }
    }
    all
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn literal_public_set_refusals_before_write() {
    let rows: Vec<&str> = PUBLIC_INSTALLED.split('\n').collect();
    let invalid = [
        rows[..3].join("\n"),
        format!("{PUBLIC_INSTALLED}\nunknown-x86_64-unknown-linux-gnu"),
        [rows[0]; 4].join("\n"),
        [rows[0], rows[1], rows[2], rows[0]].join("\n"),
        PUBLIC_INSTALLED.replace("x86_64", "aarch64"),
        PUBLIC_INSTALLED.replace("clippy-", "clippy-preview-"),
        PUBLIC_INSTALLED.replace("clippy-", "rustfmt-"),
        PUBLIC_INSTALLED.replace("clippy-", "clippy-nightly-"),
        PUBLIC_INSTALLED.replace('\n', " (installed)\n") + " (installed)",
        PUBLIC_INSTALLED.replace('\n', "\r\n"),
        PUBLIC_INSTALLED.replace('\n', "\x0b"),
        PUBLIC_INSTALLED.replace('\n', "\n\n"),
        String::new(),
    ];
    let fixture = Fixture::new();
    for text in &invalid {
        fixture.refuses_with(&mut listing(text), "public-installed-set");
        assert_eq!(fs::read(&fixture.component).unwrap(), canonical());
        assert!(!fixture.output.exists(), "{text:?}");
    }
}

#[test]
fn public_diagnostics_never_echo_unknown_output() {
    let raw =
        format!("{PUBLIC_INSTALLED}\nunknown-component https://example.invalid/private-canary\n");
    let (line, rows) = installed_rows(&raw);
    assert_eq!(rows.unwrap_err().to_string(), "public-installed-set");
    let diagnostic: Value = serde_json::from_str(&line).unwrap();
    let recognized: Vec<&str> = PUBLIC_INSTALLED.split('\n').collect();
    assert_eq!(
        diagnostic,
        json!({"rustup_installed": {"recognized": recognized, "row_count": 5,
               "unknown_count": 1, "raw_sha256": sha256_hex(raw.as_bytes())}})
    );
    for secret in ["unknown-component", "private-canary", "https://"] {
        assert!(!line.contains(secret));
    }
    // Byte form of Python's default `json.dumps`, keys in insertion order.
    let (line, _) = installed_rows("x\nx");
    assert_eq!(
        line,
        format!(
            "{{\"rustup_installed\": {{\"recognized\": [], \"row_count\": 2, \"unknown_count\": 2, \"raw_sha256\": \"{}\"}}}}",
            sha256_hex(b"x\nx")
        )
    );
}

#[test]
fn literal_public_terminal_lf_preserves_raw_hash() {
    let raw = format!("{PUBLIC_INSTALLED}\n");
    let (line, rows) = installed_rows(&raw);
    assert_eq!(
        rows.unwrap(),
        PUBLIC_INSTALLED.split('\n').collect::<Vec<_>>()
    );
    let diagnostic: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        diagnostic["rustup_installed"]["raw_sha256"],
        sha256_hex(raw.as_bytes())
    );
}

#[test]
fn diagnostic_line_is_printed_before_the_refusal() {
    let fixture = Fixture::new();
    let (result, out) = fixture.run(&mut listing("unknown"));
    assert_eq!(result.unwrap_err().to_string(), "public-installed-set");
    assert_eq!(out.lines().count(), 1);
    assert!(out.starts_with("{\"rustup_installed\": "));
}

#[test]
fn every_valid_order_preserves_tools_and_receipts() {
    let mut fixture = Fixture::new();
    for (index, order) in permutations(&ROWS).into_iter().enumerate() {
        fixture.output = fixture.parent.join(format!("receipts-{index}"));
        let raw = rows_raw(&order);
        fs::write(&fixture.component, &raw).unwrap();
        let before = closure(&fixture.root).unwrap();
        let info = fixture.component_identity();
        let (result, out) = fixture.run(&mut public());
        let result = result.unwrap();
        assert_eq!(out.lines().count(), 2);
        assert_eq!(fs::read(&fixture.component).unwrap(), canonical());
        assert_eq!(fixture.component_identity(), info);
        assert_eq!(closure(&fixture.root).unwrap(), before);
        assert_eq!(
            fs::read(fixture.output.join("original-components.txt")).unwrap(),
            raw
        );
        let receipt = read_json(&fixture.output.join("before.json"));
        assert_eq!(receipt["original_order"], json!(order));
        assert_eq!(receipt["other_toolchain_inputs"], before.to_json());
        assert_eq!(result["changed"], json!(raw != canonical()));
        assert_eq!(
            read_json(&fixture.output.join("after.json")),
            Value::Object(result)
        );
        assert_eq!(fs::metadata(&fixture.output).unwrap().mode() & 0o777, 0o700);
        let mut names = Vec::new();
        for file in fs::read_dir(&fixture.output).unwrap() {
            let file = file.unwrap();
            let info = file.metadata().unwrap();
            assert_eq!(info.mode() & 0o777, 0o600);
            assert_eq!(
                (info.uid(), info.gid()),
                (Owner::current().uid, Owner::current().gid)
            );
            names.push(file.file_name().into_string().unwrap());
        }
        names.sort();
        assert_eq!(
            names,
            ["after.json", "before.json", "original-components.txt"]
        );
    }
}

#[test]
fn receipts_have_python_indent_form() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    let raw = fs::read_to_string(fixture.output.join("after.json")).unwrap();
    assert!(
        raw.starts_with("{\n  \"canonical_order\": [\n    \"cargo-x86_64-unknown-linux-gnu\",\n")
    );
    assert!(raw.ends_with("\n}\n"));
    assert_eq!(
        raw,
        json::indented(&read_json(&fixture.output.join("after.json")))
    );
}

#[test]
fn second_invocation_new_receipt_is_no_write() {
    let mut fixture = Fixture::new();
    fs::write(&fixture.component, reversed()).unwrap();
    assert_eq!(fixture.prepare().unwrap()["changed"], json!(true));
    let modified = fs::metadata(&fixture.component).unwrap().mtime_nsec();
    let seconds = fs::metadata(&fixture.component).unwrap().mtime();
    fixture.output = fixture.parent.join("second-receipts");
    assert_eq!(fixture.prepare().unwrap()["changed"], json!(false));
    let info = fs::metadata(&fixture.component).unwrap();
    assert_eq!((info.mtime(), info.mtime_nsec()), (seconds, modified));
}

#[test]
fn literal_lf_five_reviewed_separator_refusals() {
    let fixture = Fixture::new();
    for separator in [0x0b, 0x0c, 0x1c, 0x1d, 0x1e] {
        let mut raw = canonical();
        let first = raw.iter().position(|&b| b == b'\n').unwrap();
        raw[first] = separator;
        assert_eq!(raw.len(), 136);
        fs::write(&fixture.component, &raw).unwrap();
        fixture.refuses("components-set");
        assert!(!fixture.output.exists());
    }
}

#[test]
fn malformed_rows_refuse_before_receipt_or_write() {
    let canonical = canonical();
    let replace_first = |from: &[u8], to: &[u8]| {
        let text = String::from_utf8(canonical.clone()).unwrap();
        text.replacen(
            std::str::from_utf8(from).unwrap(),
            std::str::from_utf8(to).unwrap(),
            1,
        )
        .into_bytes()
    };
    let mut high = canonical.clone();
    high[0] = 0xff;
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (canonical[..135].to_vec(), "components-byte-format"),
        ([&canonical[..], b"\n"].concat(), "components-byte-format"),
        (
            String::from_utf8(canonical.clone())
                .unwrap()
                .replace('\n', "\r\n")
                .into_bytes(),
            "components-byte-format",
        ),
        (high, "components-byte-format"),
        (replace_first(b"cargo", b"cargx"), "components-set"),
        (
            replace_first(b"x86_64", b"aarch64"),
            "components-byte-format",
        ),
        (rows_raw(&ROWS[..3]), "components-byte-format"),
        (
            [&canonical[..], b"unknown\n"].concat(),
            "components-byte-format",
        ),
        (
            rows_raw(&[ROWS[0], ROWS[1], ROWS[2], ROWS[0]]),
            "components-set",
        ),
        (
            rows_raw(&[ROWS[0], ROWS[1], ROWS[2], "rustc-x86_64-unknown-linux-gnX"]),
            "components-set",
        ),
    ];
    let fixture = Fixture::new();
    for (raw, expected) in cases {
        fs::write(&fixture.component, &raw).unwrap();
        fixture.refuses(expected);
        assert!(!fixture.output.exists());
    }
}

#[test]
fn component_symlink() {
    let fixture = Fixture::new();
    let target = fixture.parent.join("retained");
    fs::rename(&fixture.component, &target).unwrap();
    symlink(&target, &fixture.component).unwrap();
    fixture.refuses("nonphysical-path");
}

#[test]
fn ancestor_symlink() {
    let fixture = Fixture::new();
    let rustlib = fixture.root.join("lib/rustlib");
    let target = fixture.root.join("retained");
    fs::rename(&rustlib, &target).unwrap();
    symlink(&target, &rustlib).unwrap();
    fixture.refuses("nonphysical-path");
}

#[test]
fn hardlink() {
    let fixture = Fixture::new();
    fs::hard_link(&fixture.component, fixture.parent.join("hardlink")).unwrap();
    fixture.refuses("nonregular-or-hardlinked-file");
}

#[test]
fn component_mode() {
    let fixture = Fixture::new();
    mode(&fixture.component, 0o666);
    fixture.refuses("unexpected-file-mode");
}

#[test]
fn schema_refusals() {
    let fixture = Fixture::new();
    for bytes in [&b"4\n"[..], b"3\n\n", b" 3", b""] {
        fs::write(&fixture.schema, bytes).unwrap();
        fixture.refuses("unsupported-installer-schema");
    }
    fs::write(&fixture.schema, b"3").unwrap();
    assert!(fixture.prepare().is_ok());
}

#[test]
fn schema_symlink() {
    let fixture = Fixture::new();
    let target = fixture.parent.join("schema");
    fs::rename(&fixture.schema, &target).unwrap();
    symlink(&target, &fixture.schema).unwrap();
    fixture.refuses("nonphysical-path");
}

#[test]
fn missing_and_empty_manifest() {
    let fixture = Fixture::new();
    let manifest = fixture
        .root
        .join("lib/rustlib")
        .join(format!("manifest-{}", ROWS[0]));
    fs::write(&manifest, b"").unwrap();
    fixture.refuses("missing-installed-manifest");
    fs::remove_file(&manifest).unwrap();
    fixture.refuses("FileNotFoundError");
}

#[test]
fn component_directory_and_fifo() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.component).unwrap();
    fs::create_dir(&fixture.component).unwrap();
    fixture.refuses("nonregular-or-hardlinked-file");
    fs::remove_dir(&fixture.component).unwrap();
    let path = std::ffi::CString::new(fixture.component.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: path is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o644) }, 0);
    fixture.refuses("nonregular-or-hardlinked-file");
}

#[test]
fn occupied_destinations() {
    let fixture = Fixture::new();
    fs::write(&fixture.component, reversed()).unwrap();
    fs::create_dir(&fixture.output).unwrap();
    fixture.refuses("FileExistsError");
    fs::remove_dir(&fixture.output).unwrap();
    symlink(&fixture.parent, &fixture.output).unwrap();
    fixture.refuses("FileExistsError");
    fs::remove_file(&fixture.output).unwrap();
    fs::write(&fixture.output, b"owned occupied destination").unwrap();
    fixture.refuses("FileExistsError");
}

#[test]
fn receipt_failure_prevents_write() {
    let fixture = Fixture::new();
    fs::write(&fixture.component, reversed()).unwrap();
    for name in ["original-components.txt", "before.json"] {
        let _ = fs::remove_dir_all(&fixture.output);
        let mut inspect = fake(
            |_| Ok(PUBLIC_INSTALLED.to_owned()),
            |point| match point {
                Point::Receipt(at) if at == name => {
                    Err(io::Error::from_raw_os_error(libc::ENOSPC).into())
                }
                _ => Ok(()),
            },
        );
        fixture.refuses_with(&mut inspect, "OSError");
        assert!(!fixture.output.join("after.json").exists());
    }
}

#[test]
fn compiled_bytes_drift_refuses_admission() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |call| {
            if call == 2 {
                fs::write(&fixture.compiler, b"owned injected compiled byte drift").unwrap();
            }
            Ok(PUBLIC_INSTALLED.to_owned())
        },
        |_| Ok(()),
    );
    fixture.refuses_with(&mut inspect, "compiled-toolchain-input-drift");
    assert!(fixture.output.join("before.json").exists());
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn compiled_mode_drift_refuses_admission() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |call| {
            if call == 2 {
                mode(&fixture.compiler, 0o644);
            }
            Ok(PUBLIC_INSTALLED.to_owned())
        },
        |_| Ok(()),
    );
    fixture.refuses_with(&mut inspect, "compiled-toolchain-input-drift");
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn compiled_entry_added_or_removed_refuses_admission() {
    for add in [true, false] {
        let fixture = Fixture::new();
        let extra = fixture.root.join("bin/extra");
        if !add {
            fs::write(&extra, b"present before").unwrap();
        }
        let mut inspect = fake(
            |call| {
                if call == 2 {
                    if add {
                        fs::write(&extra, b"").unwrap();
                    } else {
                        fs::remove_file(&extra).unwrap();
                    }
                }
                Ok(PUBLIC_INSTALLED.to_owned())
            },
            |_| Ok(()),
        );
        fixture.refuses_with(&mut inspect, "compiled-toolchain-input-drift");
    }
}

#[test]
fn toolchain_symlink_identity_and_external_refusal() {
    let mut fixture = Fixture::new();
    let link = fixture.root.join("bin/internal-link");
    symlink("rustc", &link).unwrap();
    let result = fixture.prepare().unwrap();
    assert_eq!(result["compiled_toolchain_inputs_unchanged"], json!(true));
    let entries =
        read_json(&fixture.output.join("before.json"))["other_toolchain_inputs"]["entries"].clone();
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry[0] == "bin/internal-link" && entry[7] == "rustc"));
    for target in [
        fixture.rustup.clone(),
        PathBuf::from("missing"),
        PathBuf::from("internal-link"),
    ] {
        fs::remove_file(&link).unwrap();
        symlink(&target, &link).unwrap();
        fixture.output = fixture
            .parent
            .join(format!("receipts-{}", target.display()).replace('/', "_"));
        fixture.refuses("external-toolchain-symlink");
    }
}

#[test]
fn closure_refuses_unreadable_directories_and_other_entries() {
    let fixture = Fixture::new();
    mode(&fixture.root.join("bin"), 0o000);
    if fs::read_dir(fixture.root.join("bin")).is_err() {
        fixture.refuses("PermissionError");
    }
    mode(&fixture.root.join("bin"), 0o755);
    let fifo = std::ffi::CString::new(fixture.root.join("bin/fifo").as_os_str().as_encoded_bytes())
        .unwrap();
    // SAFETY: fifo is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    fixture.refuses("unsupported-toolchain-entry");
}

#[test]
fn closure_refuses_non_utf8_names() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(std::ffi::OsStr::from_bytes(b"bin/\xff")),
        b"",
    )
    .unwrap();
    fixture.refuses("toolchain-entry-name");
}

#[test]
fn closure_orders_by_path_component() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join("a")).unwrap();
    fs::write(fixture.root.join("a/b"), b"").unwrap();
    fs::write(fixture.root.join("a-c"), b"").unwrap();
    fs::write(fixture.root.join("é"), b"").unwrap();
    let names: Vec<String> = closure(&fixture.root)
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry[0].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "a",
            "a/b",
            "a-c",
            "bin",
            "bin/rustc",
            "lib",
            "lib/rustlib",
            "lib/rustlib/manifest-cargo-x86_64-unknown-linux-gnu",
            "lib/rustlib/manifest-clippy-preview-x86_64-unknown-linux-gnu",
            "lib/rustlib/manifest-rust-std-x86_64-unknown-linux-gnu",
            "lib/rustlib/manifest-rustc-x86_64-unknown-linux-gnu",
            "lib/rustlib/rust-installer-version",
            "é",
        ]
    );
}

#[test]
fn public_set_drift_refuses_admission() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |call| {
            Ok(if call == 1 {
                PUBLIC_INSTALLED.to_owned()
            } else {
                "unknown".to_owned()
            })
        },
        |_| Ok(()),
    );
    fixture.refuses_with(&mut inspect, "public-installed-set");
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn public_inspection_metadata_drift_refuses_final_receipt() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |call| {
            if call == 2 {
                fs::write(&fixture.component, reversed()).unwrap();
            }
            Ok(PUBLIC_INSTALLED.to_owned())
        },
        |_| Ok(()),
    );
    let (result, _) = fixture.run(&mut inspect);
    assert_eq!(
        result.unwrap_err().to_string(),
        "components-final-byte-drift"
    );
    assert_eq!(fs::read(&fixture.component).unwrap(), reversed());
    assert!(fixture.output.join("before.json").exists());
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn rustup_identity_drift_before_write() {
    let fixture = Fixture::new();
    fs::write(&fixture.component, reversed()).unwrap();
    fs::write(&fixture.rustup, b"owned changed manager bytes").unwrap();
    fixture.refuses("rustup-identity-drift");
    assert!(!fixture.output.exists());
}

#[test]
fn rustup_identity_drift_after_write() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |call| {
            if call == 2 {
                mode(&fixture.rustup, 0o700);
            }
            Ok(PUBLIC_INSTALLED.to_owned())
        },
        |_| Ok(()),
    );
    fixture.refuses_with(&mut inspect, "rustup-identity-drift");
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn fd_identity_race_before_write() {
    let fixture = Fixture::new();
    fs::write(&fixture.component, reversed()).unwrap();
    let retained = fixture.parent.join("retained-original");
    let mut inspect = fake(
        |_| Ok(PUBLIC_INSTALLED.to_owned()),
        |point| {
            if point == Point::ComponentOpen {
                fs::rename(&fixture.component, &retained).unwrap();
                fs::write(&fixture.component, reversed()).unwrap();
                mode(&fixture.component, 0o644);
            }
            Ok(())
        },
    );
    let (result, _) = fixture.run(&mut inspect);
    assert_eq!(result.unwrap_err().to_string(), "components-fd-race");
    assert_eq!(fs::read(&fixture.component).unwrap(), reversed());
    assert_eq!(fs::read(&retained).unwrap(), reversed());
    assert!(!fixture.output.exists());
}

#[test]
fn unowned_root_before_write() {
    let fixture = Fixture::new();
    let owner = Owner::current();
    for foreign in [
        Owner {
            uid: owner.uid + 123,
            gid: owner.gid,
        },
        Owner {
            uid: owner.uid,
            gid: owner.gid + 123,
        },
    ] {
        let (result, out) = fixture.run_as(foreign, &mut public());
        assert_eq!(result.unwrap_err().to_string(), "unowned-directory");
        assert!(out.is_empty());
    }
}

#[test]
fn prewrite_components_path_race() {
    let fixture = Fixture::new();
    let retained = fixture.parent.join("retained-original");
    let mut inspect = fake(
        |_| Ok(PUBLIC_INSTALLED.to_owned()),
        |point| {
            if point == Point::ReceiptWritten("before.json") {
                fs::rename(&fixture.component, &retained).unwrap();
                fs::write(&fixture.component, canonical()).unwrap();
            }
            Ok(())
        },
    );
    let (result, _) = fixture.run(&mut inspect);
    assert_eq!(result.unwrap_err().to_string(), "components-path-race");
    assert_eq!(fs::read(&retained).unwrap(), canonical());
    assert!(!fixture.output.join("after.json").exists());
}

#[test]
fn prewrite_components_byte_race() {
    let fixture = Fixture::new();
    let mut inspect = fake(
        |_| Ok(PUBLIC_INSTALLED.to_owned()),
        |point| {
            if point == Point::ReceiptWritten("before.json") {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(&fixture.component)
                    .unwrap();
                file.write_all(&reversed()).unwrap();
            }
            Ok(())
        },
    );
    let (result, _) = fixture.run(&mut inspect);
    assert_eq!(result.unwrap_err().to_string(), "components-byte-race");
    assert_eq!(fs::read(&fixture.component).unwrap(), reversed());
}

#[test]
fn component_open_never_blocks_on_a_swapped_in_fifo() {
    use std::os::unix::fs::FileTypeExt;
    let fixture = Fixture::new();
    fs::remove_file(&fixture.component).unwrap();
    let path = std::ffi::CString::new(fixture.component.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: path is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o644) }, 0);
    let opened = open_component(&fixture.component, false).unwrap();
    assert!(opened.metadata().unwrap().file_type().is_fifo());
}

/// Replaces the receipt directory at `point` by a symlink to, or a fresh
/// directory at, another location; receipts only ever land in the directory
/// that was created, and preparation refuses.
#[test]
fn receipt_directory_replacement_refuses() {
    let cases: [(Point, bool, &[&str]); 5] = [
        (Point::Receipt("original-components.txt"), false, &[]),
        (
            Point::Receipt("before.json"),
            false,
            &["original-components.txt"],
        ),
        (
            Point::ReceiptWritten("before.json"),
            false,
            &["before.json", "original-components.txt"],
        ),
        (
            Point::Receipt("after.json"),
            true,
            &["before.json", "original-components.txt"],
        ),
        (
            Point::ReceiptWritten("after.json"),
            true,
            &["after.json", "before.json", "original-components.txt"],
        ),
    ];
    for (point, written, kept) in cases {
        for link in [true, false] {
            let fixture = Fixture::new();
            fs::write(&fixture.component, reversed()).unwrap();
            let retained = fixture.parent.join("retained");
            let outside = fixture.parent.join("outside");
            let mut inspect = fake(
                |_| Ok(PUBLIC_INSTALLED.to_owned()),
                |at| {
                    if at == point {
                        fs::rename(&fixture.output, &retained).unwrap();
                        if link {
                            fs::create_dir(&outside).unwrap();
                            symlink(&outside, &fixture.output).unwrap();
                        } else {
                            fs::create_dir(&fixture.output).unwrap();
                        }
                    }
                    Ok(())
                },
            );
            let (result, _) = fixture.run(&mut inspect);
            let case = format!("{point:?} link={link}");
            assert_eq!(
                result.map(|_| ()).unwrap_err().to_string(),
                "receipt-directory-race",
                "{case}"
            );
            let expected = if written { canonical() } else { reversed() };
            assert_eq!(fs::read(&fixture.component).unwrap(), expected, "{case}");
            let replaced = if link { &outside } else { &fixture.output };
            assert_eq!(fs::read_dir(replaced).unwrap().count(), 0, "{case}");
            let mut names: Vec<String> = fs::read_dir(&retained)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            assert_eq!(names, kept, "{case}");
        }
    }
}

fn receipt_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Unlinks, renames out, rewrites, hardlinks, re-modes or replaces a written
/// receipt leaf (with a symlink, a FIFO or a fresh file holding the same
/// bytes) right after it is written; preparation refuses before the
/// components write or before success, and a FIFO never blocks the recheck.
#[test]
fn receipt_leaf_replacement_refuses() {
    use std::os::unix::fs::FileTypeExt;
    let attacks = [
        "unlink",
        "rename-out",
        "symlink",
        "fifo",
        "rewrite",
        "hardlink",
        "hardlink-rewrite",
        "chmod",
        "same-bytes-copy",
    ];
    let points = [
        (
            Point::ReceiptWritten("original-components.txt"),
            "original-components.txt",
        ),
        (
            Point::ReceiptWritten("before.json"),
            "original-components.txt",
        ),
        (Point::ReceiptWritten("before.json"), "before.json"),
        (Point::ReceiptWritten("after.json"), "before.json"),
        (Point::ReceiptWritten("after.json"), "after.json"),
    ];
    let mut failures = Vec::new();
    for (point, leaf) in points {
        for attack in attacks {
            let fixture = Fixture::new();
            fs::write(&fixture.component, reversed()).unwrap();
            let target = fixture.output.join(leaf);
            let outside = fixture.parent.join("outside");
            let mut inspect = fake(
                |_| Ok(PUBLIC_INSTALLED.to_owned()),
                |at| {
                    if at != point {
                        return Ok(());
                    }
                    let bytes = fs::read(&target).unwrap();
                    match attack {
                        "unlink" => fs::remove_file(&target).unwrap(),
                        "rename-out" => fs::rename(&target, &outside).unwrap(),
                        "symlink" => {
                            fs::write(&outside, &bytes).unwrap();
                            mode(&outside, 0o600);
                            fs::remove_file(&target).unwrap();
                            symlink(&outside, &target).unwrap();
                        }
                        "fifo" => {
                            fs::remove_file(&target).unwrap();
                            let path = CString::new(target.as_os_str().as_bytes()).unwrap();
                            // SAFETY: path is a valid NUL-terminated string.
                            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                        }
                        "rewrite" => {
                            let mut file = OpenOptions::new().write(true).open(&target).unwrap();
                            file.write_all(b"tampered-receipt\n").unwrap();
                        }
                        "hardlink" => fs::hard_link(&target, &outside).unwrap(),
                        "hardlink-rewrite" => {
                            fs::hard_link(&target, &outside).unwrap();
                            fs::write(&outside, b"tampered-via-hardlink\n").unwrap();
                        }
                        "chmod" => mode(&target, 0o644),
                        "same-bytes-copy" => {
                            fs::remove_file(&target).unwrap();
                            let mut file = OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .mode(0o600)
                                .open(&target)
                                .unwrap();
                            file.write_all(&bytes).unwrap();
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
            );
            let (result, _) = fixture.run(&mut inspect);
            let expected = if point == Point::ReceiptWritten("after.json") {
                canonical()
            } else {
                reversed()
            };
            let reason = result.map(|_| ()).map_err(|refusal| refusal.to_string());
            let components = fs::read(&fixture.component).unwrap() == expected;
            let fifo_left = fs::symlink_metadata(&target)
                .is_ok_and(|info| info.file_type().is_fifo())
                == (attack == "fifo");
            let after_absent = point == Point::ReceiptWritten("after.json")
                || !fixture.output.join("after.json").exists();
            if reason != Err("receipt-leaf-race".to_owned())
                || !components
                || !fifo_left
                || !after_absent
            {
                failures.push(format!(
                    "{point:?} {leaf} {attack}: {reason:?} components_ok={components} fifo_left={fifo_left} after_absent={after_absent}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A name planted next to the receipts refuses before the components write.
#[test]
fn receipt_directory_extra_entry_refuses() {
    for point in [
        Point::Receipt("original-components.txt"),
        Point::ReceiptWritten("original-components.txt"),
        Point::Receipt("before.json"),
        Point::ReceiptWritten("before.json"),
    ] {
        let fixture = Fixture::new();
        fs::write(&fixture.component, reversed()).unwrap();
        let mut inspect = fake(
            |_| Ok(PUBLIC_INSTALLED.to_owned()),
            |at| {
                if at == point {
                    fs::write(fixture.output.join("planted"), b"extra\n").unwrap();
                }
                Ok(())
            },
        );
        let (result, _) = fixture.run(&mut inspect);
        assert_eq!(
            result.map(|_| ()).unwrap_err().to_string(),
            "receipt-directory-race",
            "{point:?}"
        );
        assert_eq!(
            fs::read(&fixture.component).unwrap(),
            reversed(),
            "{point:?}"
        );
        assert!(
            !receipt_names(&fixture.output).contains(&"after.json".to_owned()),
            "{point:?}"
        );
    }
}
