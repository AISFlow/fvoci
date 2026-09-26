//! The Markdown child (`fvoci-server --internal-markdown`) as the server runs
//! it: real process, real rlimits, the TS oracle corpus and hostile inputs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fvoci_server::documents::markdown_helper::{
    MarkdownError, MarkdownHelper, MarkdownLimits, MarkdownOp,
};
use serde_json::{json, Value};

fn helper() -> MarkdownHelper {
    MarkdownHelper::new(env!("CARGO_BIN_EXE_fvoci-server"))
}

fn oracle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("compat/fixtures/markdown-oracle")
}

fn corpus() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(oracle_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".md") && !name.ends_with(".roundtrip.md")
        })
        .map(|p| {
            let stem = p.file_stem().unwrap().to_str().unwrap().to_string();
            (stem, std::fs::read_to_string(&p).unwrap())
        })
        .collect();
    out.sort();
    assert!(out.len() >= 30, "oracle corpus missing: {}", out.len());
    out
}

fn expected(name: &str, ext: &str) -> String {
    std::fs::read_to_string(oracle_dir().join(format!("{name}.{ext}"))).unwrap()
}

/// Every product operation through the child equals the TS oracle outputs
/// (`mdToTiptapJson`, `tiptapDocToSafeHtml(mdToTiptapJson)`, and
/// `tiptapDocToMd` on the oracle JSON, including its `$` self-check).
#[tokio::test]
async fn oracle_corpus_through_the_child() {
    let helper = helper();
    for (name, md) in corpus() {
        let want: Value = serde_json::from_str(&expected(&name, "json")).unwrap();
        assert_eq!(helper.md_to_tiptap(&md).await.unwrap(), want, "{name} json");
        assert_eq!(
            helper.md_to_safe_html(&md).await.unwrap(),
            expected(&name, "html"),
            "{name} html"
        );
        assert_eq!(
            helper.tiptap_to_md(&want).await.unwrap(),
            expected(&name, "roundtrip.md"),
            "{name} md"
        );
    }
}

#[tokio::test]
async fn non_doc_bodies_read_as_empty_markdown_without_a_child() {
    let missing = MarkdownHelper::new("/nonexistent/fvoci-server");
    for body in [
        json!(null),
        json!([]),
        json!({"type": "paragraph"}),
        json!({"type": "doc", "content": {}}),
    ] {
        assert_eq!(missing.tiptap_to_md(&body).await.unwrap(), "", "{body}");
    }
    // A real doc does need the child: a missing binary is a helper failure.
    let err = missing
        .tiptap_to_md(&json!({"type": "doc", "content": []}))
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::Failed(_)), "{err:?}");
}

#[tokio::test]
async fn deep_nesting_is_invalid_input_not_a_crash() {
    let helper = helper();
    assert!(helper.md_to_tiptap(&("> ".repeat(60) + "x")).await.is_ok());
    for md in ["> ".repeat(61) + "x", "> ".repeat(5_000) + "x"] {
        let err = helper.md_to_tiptap(&md).await.unwrap_err();
        assert!(matches!(err, MarkdownError::InvalidInput(_)), "{err:?}");
        let err = helper.md_to_safe_html(&md).await.unwrap_err();
        assert!(matches!(err, MarkdownError::InvalidInput(_)), "{err:?}");
    }
}

/// The watchdog kills a child that is still parsing and reports the input as
/// invalid; the parent returns promptly.
#[tokio::test]
async fn super_linear_input_hits_the_watchdog() {
    let helper = helper().with_limits(MarkdownLimits {
        timeout: Duration::from_secs(1),
        ..MarkdownLimits::default()
    });
    // Nested emphasis is super-linear in micromark and markdown-rs alike.
    let md = "*a ".repeat(60_000) + &"b* ".repeat(60_000);
    let started = Instant::now();
    let err = helper.md_to_tiptap(&md).await.unwrap_err();
    assert!(matches!(err, MarkdownError::InvalidInput(_)), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn input_and_output_caps() {
    let small = helper().with_limits(MarkdownLimits {
        max_input_bytes: 16,
        ..MarkdownLimits::default()
    });
    let err = small.md_to_tiptap(&"x".repeat(17)).await.unwrap_err();
    assert!(matches!(err, MarkdownError::TooLarge), "{err:?}");
    assert!(small.md_to_tiptap(&"x".repeat(16)).await.is_ok());

    let tight = helper().with_limits(MarkdownLimits {
        max_output_bytes: 64,
        ..MarkdownLimits::default()
    });
    let err = tight.md_to_tiptap(&"- a\n".repeat(20)).await.unwrap_err();
    assert!(matches!(err, MarkdownError::TooLarge), "{err:?}");
}

#[tokio::test]
async fn child_refuses_bad_arguments_and_non_utf8_input() {
    let err = helper()
        .run(MarkdownOp::MdToTiptap, vec![0xff, 0xfe])
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::InvalidInput(_)), "{err:?}");
    let err = helper()
        .run(MarkdownOp::TiptapToMd, b"not json".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::InvalidInput(_)), "{err:?}");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"))
        .args(["--internal-markdown", "--op", "nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing --op"));
}
