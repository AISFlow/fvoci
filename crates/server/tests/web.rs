//! Integration tests for serving the static web app directory and client-side routes.

use std::path::PathBuf;

use axum::body::{Body, to_bytes};
use axum::http::header::{
    ACCEPT_ENCODING, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_RANGE,
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH, RANGE, REFERRER_POLICY,
    STRICT_TRANSPORT_SECURITY, VARY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use fvoci_model::new_id;
use fvoci_server::web::WebRoot;
use tower::ServiceExt as _;

struct Fixture {
    dir: PathBuf,
    web: WebRoot,
}

impl Fixture {
    fn new(server_url: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("web-tests")
            .join(new_id());
        std::fs::create_dir_all(dir.join("static/sub")).expect("the fixture directory");
        let write = |name: &str, content: &str| {
            std::fs::write(dir.join(name), content).expect("a fixture file");
        };
        write(
            "index.html",
            "<script>window.baseURL = '{{.BaseURL}}'</script><script src=\"{{.BaseURL}}/static/main.js\"></script>",
        );
        write("static/main.js", "console.log('plain')");
        write("static/app-0123456789abcdef.js", "console.log('hashed')");
        write("static/app-0123456789abcdef.js.br", "BROTLI-BYTES");
        write("static/app-0123456789abcdef.js.gz", "GZIP-BYTES");
        write("static/app-0123456789abcdef.js.zst", "ZSTD-BYTES");
        write("static/sub/nested.css", "body{}");
        write(
            "static/viewer-frame.html",
            "<!doctype html><title>frame</title>",
        );
        write("secret.txt", "outside static");
        let web = WebRoot::new(&dir, server_url);
        Self { dir, web }
    }

    async fn get(&self, path: &str, headers: &[(axum::http::HeaderName, &str)]) -> Response {
        let mut request = Request::get(format!("/static/{path}"));
        for (name, value) in headers {
            request = request.header(name, *value);
        }
        let request = request.body(Body::empty()).expect("a valid request");
        self.web.serve_static(path, request).await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a readable body");
    String::from_utf8(bytes.to_vec()).expect("a UTF-8 body")
}

fn header<'r>(response: &'r Response, name: &axum::http::HeaderName) -> &'r str {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("the response has no {name} header"))
        .to_str()
        .expect("an ASCII header value")
}

#[tokio::test]
async fn static_file_without_a_content_hash_must_be_revalidated() {
    let fixture = Fixture::new("http://localhost:8000");

    let response = fixture.get("main.js", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, &CACHE_CONTROL), "no-cache");
    assert!(
        header(&response, &CONTENT_TYPE).contains("javascript"),
        "a script content type"
    );
    let etag = header(&response, &ETAG).to_owned();
    assert_eq!(body_text(response).await, "console.log('plain')");

    // The validator from the first response makes the second one a 304.
    let again = fixture.get("main.js", &[(IF_NONE_MATCH, &etag)]).await;
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header(&again, &CACHE_CONTROL), "no-cache");
}

/// The file viewer's frame has an opaque origin: it reads the app's files across origins.
#[tokio::test]
async fn static_files_may_be_read_from_any_origin() {
    let fixture = Fixture::new("http://localhost:8000");

    for path in ["main.js", "app-0123456789abcdef.js", "sub/nested.css"] {
        let response = fixture.get(path, &[]).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            header(&response, &ACCESS_CONTROL_ALLOW_ORIGIN),
            "*",
            "{path}"
        );
        assert!(
            response.headers().get(CONTENT_SECURITY_POLICY).is_none(),
            "{path} is not sandboxed"
        );
    }
}

/// Opened on its own, the viewer's frame is sandboxed as it is in the app's iframe, by
/// whatever name reaches it.
#[tokio::test]
async fn the_viewer_frame_is_sandboxed() {
    let fixture = Fixture::new("http://localhost:8000");

    for path in [
        "viewer-frame.html",
        "viewer%2Dframe.html",
        "viewer-frame%2Ehtml",
    ] {
        let response = fixture.get(path, &[]).await;

        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            header(&response, &CONTENT_SECURITY_POLICY),
            "sandbox allow-scripts",
            "{path}"
        );
        assert_eq!(
            header(&response, &ACCESS_CONTROL_ALLOW_ORIGIN),
            "*",
            "{path}"
        );
        assert_eq!(header(&response, &CACHE_CONTROL), "no-cache", "{path}");
    }
}

#[tokio::test]
async fn static_file_with_a_content_hash_is_immutable() {
    let fixture = Fixture::new("http://localhost:8000");

    let response = fixture.get("app-0123456789abcdef.js", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, &CACHE_CONTROL),
        "public, max-age=31536000, immutable"
    );
    assert!(
        response.headers().get(CONTENT_ENCODING).is_none(),
        "no encoding was asked for"
    );
    assert_eq!(body_text(response).await, "console.log('hashed')");
}

#[tokio::test]
async fn precompressed_variants_follow_accept_encoding() {
    let fixture = Fixture::new("http://localhost:8000");

    let zstd = fixture
        .get(
            "app-0123456789abcdef.js",
            &[(ACCEPT_ENCODING, "gzip, deflate, br, zstd")],
        )
        .await;
    assert_eq!(header(&zstd, &CONTENT_ENCODING), "zstd", "zstd on a tie");
    assert_eq!(body_text(zstd).await, "ZSTD-BYTES");
    let preferred = fixture
        .get(
            "app-0123456789abcdef.js",
            &[(ACCEPT_ENCODING, "zstd;q=0.5, br")],
        )
        .await;
    assert_eq!(
        header(&preferred, &CONTENT_ENCODING),
        "br",
        "the client's preference"
    );

    let brotli = fixture
        .get("app-0123456789abcdef.js", &[(ACCEPT_ENCODING, "gzip, br")])
        .await;
    assert_eq!(header(&brotli, &CONTENT_ENCODING), "br");
    assert!(
        header(&brotli, &VARY)
            .to_lowercase()
            .contains("accept-encoding"),
        "Vary names the header"
    );
    assert!(
        header(&brotli, &CONTENT_TYPE).contains("javascript"),
        "the type of the original file"
    );
    assert_eq!(
        header(&brotli, &CACHE_CONTROL),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(body_text(brotli).await, "BROTLI-BYTES");

    let gzip = fixture
        .get("app-0123456789abcdef.js", &[(ACCEPT_ENCODING, "gzip")])
        .await;
    assert_eq!(header(&gzip, &CONTENT_ENCODING), "gzip");
    assert_eq!(body_text(gzip).await, "GZIP-BYTES");

    // A file without a compressed variant is served as it is.
    let plain = fixture
        .get("main.js", &[(ACCEPT_ENCODING, "gzip, br")])
        .await;
    assert!(
        plain.headers().get(CONTENT_ENCODING).is_none(),
        "there is no variant to serve"
    );
    assert_eq!(body_text(plain).await, "console.log('plain')");
}

#[tokio::test]
async fn range_requests_get_partial_content() {
    let fixture = Fixture::new("http://localhost:8000");

    let response = fixture.get("main.js", &[(RANGE, "bytes=0-6")]).await;

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&response, &CONTENT_RANGE), "bytes 0-6/20");
    assert_eq!(body_text(response).await, "console");
}

#[tokio::test]
async fn what_is_not_a_file_under_static_is_not_found() {
    let fixture = Fixture::new("http://localhost:8000");

    for path in [
        "missing.js",
        "sub",
        "sub/",
        "",
        "../secret.txt",
        "..%2Fsecret.txt",
        "sub/../../secret.txt",
    ] {
        let response = fixture.get(path, &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "path {path:?}");
        assert!(
            response.headers().get(CACHE_CONTROL).is_none(),
            "a 404 carries no cache policy"
        );
        assert_eq!(body_text(response).await, "not found\n", "path {path:?}");
    }

    let nested = fixture.get("sub/nested.css", &[]).await;
    assert_eq!(nested.status(), StatusCode::OK);
}

#[tokio::test]
async fn index_gets_the_base_url_and_must_be_revalidated() {
    let fixture = Fixture::new("https://example.com/boards");
    assert_eq!(fixture.web.base_url(), "/boards");

    let response = fixture.web.serve_index().await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, &CONTENT_TYPE), "text/html; charset=utf-8");
    assert_eq!(header(&response, &CACHE_CONTROL), "no-cache");
    assert_eq!(header(&response, &X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert_eq!(header(&response, &X_FRAME_OPTIONS), "DENY");
    assert_eq!(
        header(&response, &CONTENT_SECURITY_POLICY),
        strict_policy("script-src 'self'"),
        "no csp.json: the strict policy, no inline script"
    );
    assert_eq!(
        body_text(response).await,
        "<script>window.baseURL = '/boards'</script><script src=\"/boards/static/main.js\"></script>"
    );
}

/// The strict policy of `index.html`, with `script_src` as its `script-src`.
fn strict_policy(script_src: &str) -> String {
    format!(
        "default-src 'self'; {script_src}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' blob:; font-src 'self'; connect-src 'self'; object-src 'none'; \
         base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    )
}

/// The policy of `index.html` takes the hashes of the inline scripts from `csp.json`;
/// without the file, or with one that is not as the build writes it, it is the same
/// strict policy with none, never one without `script-src`.
#[tokio::test]
async fn the_index_policy_is_strict_with_or_without_csp_json() {
    let fixture = Fixture::new("http://localhost:8000");
    let hash = "'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='";
    for (csp_json, script_src) in [
        (None, "script-src 'self'".to_owned()),
        (
            Some(format!(r#"{{"script-src":["{hash}"]}}"#)),
            format!("script-src 'self' {hash}"),
        ),
        (Some("not json".to_owned()), "script-src 'self'".to_owned()),
        (
            Some(r#"{"script-src":["'unsafe-inline'"]}"#.to_owned()),
            "script-src 'self'".to_owned(),
        ),
        (
            Some(r#"{"style-src":[]}"#.to_owned()),
            "script-src 'self'".to_owned(),
        ),
    ] {
        let path = fixture.dir.join("csp.json");
        match &csp_json {
            Some(content) => std::fs::write(&path, content).expect("csp.json is written"),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        let response = fixture.web.serve_index().await;
        assert_eq!(response.status(), StatusCode::OK, "{csp_json:?}");
        let policy = header(&response, &CONTENT_SECURITY_POLICY);
        assert_eq!(policy, strict_policy(&script_src), "{csp_json:?}");
        assert!(
            policy.starts_with("default-src 'self'; script-src 'self'"),
            "{csp_json:?}"
        );
    }
}

#[tokio::test]
async fn missing_index_is_a_server_error() {
    let fixture = Fixture::new("http://localhost:8000");
    std::fs::remove_file(fixture.dir.join("index.html")).expect("the index can be removed");

    let response = fixture.web.serve_index().await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body_text(response).await, "");
}

#[tokio::test]
async fn router_sets_nosniff_clickjacking_headers_and_conditional_hsts_without_duplicates() {
    let fixture = Fixture::new("http://localhost:8000");
    for (server_url, expect_hsts) in [
        ("http://localhost:8000", false),
        ("https://boards.example.com", true),
    ] {
        let data_dir = fixture.dir.join(new_id());
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db_path = data_dir.join("test.db");
        let mut cfg = fvoci_server::config::Configuration::default();
        server_url.clone_into(&mut cfg.server_url);
        cfg.web_app_path = fixture.dir.display().to_string();
        cfg.database_kind = "sqlite".to_owned();
        cfg.database_connection = db_path.display().to_string();
        cfg.file_storage_path = data_dir.join("files").display().to_string();
        cfg.skip_template_init = true;
        if expect_hsts {
            cfg.trusted_proxies = vec!["127.0.0.1/32".to_owned()];
        }
        let server = fvoci_server::Server::new(cfg, "")
            .await
            .expect("server built");
        let router = server.router();

        for path in [
            "/",
            "/api/v1/clientConfig",
            "/static/main.js",
            "/api/v1/missing",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("valid request"),
                )
                .await
                .expect("infallible router");
            let nosniff: Vec<_> = response
                .headers()
                .get_all(X_CONTENT_TYPE_OPTIONS)
                .iter()
                .collect();
            assert_eq!(nosniff.len(), 1, "single nosniff header on {path}");
            assert_eq!(nosniff[0], "nosniff", "{path}");
            assert_eq!(header(&response, &REFERRER_POLICY), "no-referrer", "{path}");
            if expect_hsts {
                assert_eq!(
                    header(&response, &STRICT_TRANSPORT_SECURITY),
                    "max-age=31536000; includeSubDomains",
                    "HSTS on {path}"
                );
            } else {
                assert!(
                    response.headers().get(STRICT_TRANSPORT_SECURITY).is_none(),
                    "plain HTTP must not send HSTS on {path}"
                );
            }
            if path == "/" {
                assert_eq!(header(&response, &X_FRAME_OPTIONS), "DENY");
                assert_eq!(
                    header(&response, &CONTENT_SECURITY_POLICY),
                    strict_policy("script-src 'self'"),
                    "no csp.json: the strict policy, no inline script"
                );
            }
        }
        server.shutdown().await;
    }
}
