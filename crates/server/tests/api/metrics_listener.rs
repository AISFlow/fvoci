//! The metrics listener faced with what anyone who reaches it may send: unknown paths,
//! methods and headers make no series and are not repeated, and the bearer token is
//! taken from one `Authorization` header only, never echoed.

use std::collections::BTreeSet;
use std::time::Duration;

use reqwest::{Method, StatusCode};

use crate::common::{Client, Setup, TestServer};
use crate::metrics::scrape;

const TOKEN: &str = "independent-review-token-32-characters";

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("an HTTP client")
}

fn metrics_url(server: &TestServer) -> String {
    format!(
        "http://{}/metrics",
        server.metrics_address.expect("a metrics listener")
    )
}

/// The series of the requests and the problems, without their values.
fn request_series(text: &str) -> BTreeSet<&str> {
    text.lines()
        .filter(|line| {
            line.starts_with("fvoci_http_request_") || line.starts_with("fvoci_http_problems_")
        })
        .filter_map(|line| line.split_once(' ').map(|(series, _)| series))
        .collect()
}

/// Sends `count` rounds of long unknown paths under the API and the problems, with
/// unknown methods and a header, and returns the marker of each round.
async fn hostile_requests(server: &TestServer, count: usize) -> Vec<String> {
    let mut inputs = Vec::new();
    for index in 0..count {
        let marker = format!("untrusted-{}-{}", fvoci_model::new_id(), "x".repeat(2048));
        let method = Method::from_bytes(format!("UNTRUSTED{index}").as_bytes()).expect("a method");
        for path in [format!("/api/v1/{marker}"), format!("/problems/{marker}")] {
            Client::send(
                server
                    .client
                    .raw_request(method.clone(), &path)
                    .header("x-review-marker", &marker),
            )
            .await;
            Client::send(server.client.raw_request(Method::GET, &path)).await;
        }
        inputs.push(marker);
    }
    inputs
}

#[tokio::test]
async fn long_unknown_paths_methods_problems_and_headers_do_not_add_series() {
    let server = Setup::native().metrics_at("127.0.0.1:0").start().await;
    hostile_requests(&server, 1).await;
    let before = scrape(&server).await;
    let inputs = hostile_requests(&server, 24).await;
    let after = scrape(&server).await;
    server.teardown().await;
    assert_eq!(
        request_series(&before),
        request_series(&after),
        "no new series"
    );
    for input in inputs {
        assert!(
            !after.contains(&input),
            "no request input in the exposition"
        );
    }
}

#[tokio::test]
async fn bearer_header_shapes_and_long_guesses_do_not_leak_tokens() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| TOKEN.clone_into(&mut config.metrics_token))
        .start()
        .await;
    let cases = [
        (format!("Bearer {TOKEN}"), StatusCode::OK),
        (format!("bearer   {TOKEN}"), StatusCode::OK),
        (format!(" Bearer {TOKEN} "), StatusCode::OK),
        (format!("Bearer\t{TOKEN}"), StatusCode::UNAUTHORIZED),
        ("Bearer ".to_owned(), StatusCode::UNAUTHORIZED),
        ("Bearer".to_owned(), StatusCode::UNAUTHORIZED),
        (
            format!("Bearer {}", "g".repeat(16384)),
            StatusCode::UNAUTHORIZED,
        ),
        (format!("Bearer {TOKEN}x"), StatusCode::UNAUTHORIZED),
    ];
    for (authorization, expected) in cases {
        let response = http()
            .get(metrics_url(&server))
            .header("authorization", authorization)
            .send()
            .await
            .expect("an authorization response");
        assert_eq!(response.status(), expected, "the bearer header's status");
        let body = response.text().await.expect("the answer text");
        assert!(!body.contains(TOKEN), "the token is never echoed");
        if expected == StatusCode::UNAUTHORIZED {
            assert!(!body.contains("fvoci_build_info"), "no metrics in refusals");
        }
    }
    server.teardown().await;
}

#[tokio::test]
async fn conflicting_authorization_headers_are_rejected_in_either_order() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| TOKEN.clone_into(&mut config.metrics_token))
        .start()
        .await;
    let valid = format!("Bearer {TOKEN}");
    let mut answers = Vec::new();
    for headers in [
        [valid.as_str(), "Bearer wrong"],
        ["Bearer wrong", valid.as_str()],
        [valid.as_str(), valid.as_str()],
    ] {
        let response = http()
            .get(metrics_url(&server))
            .header("authorization", headers[0])
            .header("authorization", headers[1])
            .send()
            .await
            .expect("an answer to duplicate headers");
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let (challenge, content_type) = (header("www-authenticate"), header("content-type"));
        let status = response.status();
        let body = response.text().await.expect("the answer text");
        assert_eq!(content_type, "application/problem+json", "{body}");
        assert!(!body.contains(TOKEN), "the token is never echoed");
        answers.push((status, challenge));
    }
    server.teardown().await;
    let refused = (
        StatusCode::UNAUTHORIZED,
        "Bearer realm=\"metrics\", error=\"invalid_request\"".to_owned(),
    );
    assert_eq!(
        answers,
        [refused.clone(), refused.clone(), refused],
        "conflicting credentials must not depend on header order"
    );
}
