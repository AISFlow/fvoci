//! Behavior of the file backends: shared checks run against both the local filesystem
//! backend and an S3-compatible server.
//!
//! The S3 test is ignored by default. To run it, start an S3-compatible server, create a
//! bucket, set `FVOCI_S3_TEST_ENDPOINT` (host:port), `FVOCI_S3_TEST_BUCKET` and, if the
//! server checks them, `FVOCI_S3_TEST_ACCESS_KEY`, `FVOCI_S3_TEST_SECRET_KEY`,
//! `FVOCI_S3_TEST_REGION` and `FVOCI_S3_TEST_SSL=true`, and pass `--ignored`.

use std::path::PathBuf;

use bytes::Bytes;
use futures_util::TryStreamExt as _;
use fvoci_model::new_id;
use fvoci_server::config::S3Config;
use fvoci_server::storage::{FileBackend, LocalFiles, S3Files, StorageError};

async fn write(backend: &FileBackend, path: &str, content: &[u8]) -> u64 {
    let mut reader = content;
    backend
        .write(path, &mut reader)
        .await
        .unwrap_or_else(|error| panic!("writing {path}: {error}"))
}

async fn read(backend: &FileBackend, path: &str, range: Option<std::ops::Range<u64>>) -> Vec<u8> {
    let stream = backend
        .read(path, range)
        .await
        .unwrap_or_else(|error| panic!("opening {path}: {error}"));
    let chunks: Vec<bytes::Bytes> = stream
        .try_collect()
        .await
        .unwrap_or_else(|error| panic!("reading {path}: {error}"));
    chunks.concat()
}

/// Asserts that the file at `path` holds exactly `want`, read whole or by `range`.
async fn assert_content(
    backend: &FileBackend,
    path: &str,
    range: Option<std::ops::Range<u64>>,
    want: &[u8],
) {
    let got = read(backend, path, range.clone()).await;
    assert!(
        got == want,
        "{path} range {range:?}: got {} bytes, want {}",
        got.len(),
        want.len()
    );
}

/// The checks every backend must pass. `dir` is a path prefix unique to this run.
async fn exercise(backend: &FileBackend, dir: &str) {
    let path = format!("{dir}/0b9f3c1e/7abc/s3cr3t");
    read_and_write(backend, &path, dir).await;
    copy_and_rename(backend, &path, dir).await;
    large_file(backend, dir).await;
    refused_paths(backend, &path).await;
}

async fn read_and_write(backend: &FileBackend, path: &str, dir: &str) {
    // Nothing is there yet.
    let size = backend.size(path).await.expect("size of a missing file");
    assert_eq!(size, None, "a missing file has no size");
    assert!(
        !backend.exists(path).await.expect("exists"),
        "{path} should not exist yet"
    );
    let missing = backend
        .read(path, None)
        .await
        .err()
        .expect("reading a missing file fails");
    assert!(
        matches!(missing, StorageError::NotFound(_)),
        "got {missing:?}"
    );

    // Write creates the directories on the way and reports the size.
    let content = b"0123456789abcdefghij";
    assert_eq!(write(backend, path, content).await, 20, "bytes written");
    assert_eq!(
        backend.size(path).await.expect("size"),
        Some(20),
        "size after the write"
    );
    assert!(
        backend.exists(path).await.expect("exists"),
        "{path} should exist"
    );
    assert_content(backend, path, None, content).await;

    // Ranges are byte offsets, end exclusive.
    assert_content(backend, path, Some(0..5), b"01234").await;
    assert_content(backend, path, Some(10..20), b"abcdefghij").await;
    assert_content(backend, path, Some(19..20), b"j").await;

    // Writing again replaces the file.
    assert_eq!(
        write(backend, path, b"new").await,
        3,
        "bytes written by the rewrite"
    );
    assert_content(backend, path, None, b"new").await;

    // An empty file is a file.
    let empty = format!("{dir}/empty.bin");
    assert_eq!(
        write(backend, &empty, b"").await,
        0,
        "bytes written for an empty file"
    );
    assert_eq!(
        backend.size(&empty).await.expect("size"),
        Some(0),
        "size of an empty file"
    );
    assert_content(backend, &empty, None, b"").await;
}

/// Expects `path` to hold `new`, as [`read_and_write`] leaves it.
async fn copy_and_rename(backend: &FileBackend, path: &str, dir: &str) {
    // Copy keeps the source; the target may be in a directory that does not exist yet.
    let copy = format!("{dir}/0/board-1/7copy.png");
    backend.copy(path, &copy).await.expect("copy");
    assert_content(backend, &copy, None, b"new").await;
    assert_content(backend, path, None, b"new").await;

    // Rename moves the file.
    let moved = format!("{dir}/0/board-2/7moved.png");
    backend.rename(&copy, &moved).await.expect("rename");
    assert_content(backend, &moved, None, b"new").await;
    assert!(
        !backend.exists(&copy).await.expect("exists"),
        "the source of a rename is gone"
    );

    // Copying or moving a missing file fails as not found.
    let gone = format!("{dir}/gone.png");
    let error = backend
        .copy(&gone, &copy)
        .await
        .expect_err("copy of a missing file");
    assert!(matches!(error, StorageError::NotFound(_)), "got {error:?}");
    let error = backend
        .rename(&gone, &copy)
        .await
        .expect_err("rename of a missing file");
    assert!(matches!(error, StorageError::NotFound(_)), "got {error:?}");
}

/// A large file survives the streaming write path.
async fn large_file(backend: &FileBackend, dir: &str) {
    let large: Vec<u8> = (0..3_000_000_u32)
        .map(|n| u8::try_from(n % 251).expect("a remainder below 251 fits a byte"))
        .collect();
    let path = format!("{dir}/large.bin");
    assert_eq!(
        write(backend, &path, &large).await,
        3_000_000,
        "bytes written for the large file"
    );
    assert_content(
        backend,
        &path,
        Some(2_999_990..3_000_000),
        &large[2_999_990..],
    )
    .await;
    assert_content(backend, &path, None, &large).await;
}

/// Paths that could leave the storage root are refused, and so is another spelling of
/// a place, also of one that holds an object.
async fn refused_paths(backend: &FileBackend, existing: &str) {
    let aliases = [
        format!("./{existing}"),
        existing.replacen('/', "//", 1),
        format!("{existing}/"),
    ];
    let leaving = ["", "/etc/passwd", "../outside.png", "a/../../outside.png"];
    let all = leaving
        .into_iter()
        .chain(aliases.iter().map(String::as_str));
    for bad in all {
        let error = backend.size(bad).await.expect_err(bad);
        assert!(
            matches!(error, StorageError::InvalidPath(_)),
            "{bad:?}: got {error:?}"
        );
        let mut reader: &[u8] = b"x";
        let error = backend.write(bad, &mut reader).await.expect_err(bad);
        assert!(
            matches!(error, StorageError::InvalidPath(_)),
            "{bad:?}: got {error:?}"
        );
        let error = backend.copy(existing, bad).await.expect_err(bad);
        assert!(
            matches!(error, StorageError::InvalidPath(_)),
            "{bad:?}: got {error:?}"
        );
    }
}

#[tokio::test]
async fn local_backend() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("storage-tests")
        .join(new_id());
    let backend = FileBackend::Local(LocalFiles::new(&root));

    exercise(&backend, "run").await;

    // The local backend lays files out exactly as given, under its root.
    assert!(
        root.join("run/0b9f3c1e/7abc/s3cr3t").is_file(),
        "the file is under the root"
    );
    // A file name is taken literally: no part of it is encoded.
    write(&backend, "run/a b#1.png", b"x").await;
    assert!(
        root.join("run/a b#1.png").is_file(),
        "the name is kept as it is"
    );

    std::fs::remove_dir_all(&root).expect("the test directory can be removed");
}

fn s3_config() -> S3Config {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    S3Config {
        access_key_id: std::env::var("FVOCI_S3_TEST_ACCESS_KEY")
            .unwrap_or_else(|_| "any".to_owned()),
        secret_access_key: std::env::var("FVOCI_S3_TEST_SECRET_KEY")
            .unwrap_or_else(|_| "any".to_owned()),
        bucket: var("FVOCI_S3_TEST_BUCKET"),
        key_prefix: "storage-tests".to_owned(),
        region: std::env::var("FVOCI_S3_TEST_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
        endpoint: var("FVOCI_S3_TEST_ENDPOINT"),
        https: var("FVOCI_S3_TEST_SSL") == "true",
        ..S3Config::default()
    }
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see the module documentation"]
async fn s3_backend() {
    let config = s3_config();
    assert!(
        !config.endpoint.is_empty() && !config.bucket.is_empty(),
        "set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET to run this test"
    );
    let backend = FileBackend::S3(S3Files::new(&config).expect("the S3 settings are usable"));

    // Objects of earlier runs stay in the bucket; a fresh prefix keeps runs apart.
    exercise(&backend, &new_id()).await;
}

/// An upload made part by part across requests: parts sent in any order, and again,
/// make the object in the order of their indexes; an aborted one makes nothing and
/// takes no more parts.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see the module documentation"]
async fn s3_multipart_upload() {
    let config = s3_config();
    assert!(
        !config.endpoint.is_empty() && !config.bucket.is_empty(),
        "set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET to run this test"
    );
    let files = S3Files::new(&config).expect("the S3 settings are usable");
    let backend = FileBackend::S3(files.clone());
    let dir = new_id();
    // Every part but the last has at least 5 MiB.
    let first: Vec<u8> = (0..5 * 1024 * 1024)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect();
    let last = b"the end".to_vec();

    let path = format!("{dir}/whole.bin");
    let upload_id = files
        .create_multipart(&path)
        .await
        .expect("the upload starts");
    let last_id = files
        .put_part(&path, &upload_id, 1, vec![Bytes::from(last.clone())])
        .await
        .expect("the last part is taken");
    files
        .put_part(&path, &upload_id, 0, vec![Bytes::from_static(b"replaced")])
        .await
        .expect("a part is taken");
    let first_id = files
        .put_part(&path, &upload_id, 0, vec![Bytes::from(first.clone())])
        .await
        .expect("a part is taken again");
    assert_eq!(
        backend.size(&path).await.expect("the size is read"),
        None,
        "nothing is there before the upload is completed"
    );
    files
        .complete_multipart(&path, &upload_id, vec![first_id, last_id])
        .await
        .expect("the upload completes");
    let mut whole = first;
    whole.extend_from_slice(&last);
    assert_content(&backend, &path, None, &whole).await;

    let path = format!("{dir}/aborted.bin");
    let upload_id = files
        .create_multipart(&path)
        .await
        .expect("the upload starts");
    files
        .put_part(&path, &upload_id, 0, vec![Bytes::from_static(b"abc")])
        .await
        .expect("a part is taken");
    files
        .abort_multipart(&path, &upload_id)
        .await
        .expect("the upload is aborted");
    files
        .abort_multipart(&path, &upload_id)
        .await
        .expect("an upload aborted already is left aborted");
    assert!(
        files
            .put_part(&path, &upload_id, 0, vec![Bytes::from_static(b"abc")])
            .await
            .is_err(),
        "an aborted upload takes no more parts"
    );
    assert_eq!(backend.size(&path).await.expect("the size is read"), None);
}

#[test]
fn s3_settings_are_checked() {
    let error = S3Files::new(&S3Config::default()).expect_err("no bucket");
    assert!(matches!(error, StorageError::Config(_)), "got {error:?}");
}
