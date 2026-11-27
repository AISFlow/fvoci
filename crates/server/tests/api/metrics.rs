//! The Prometheus metrics: kept only if `metrics_address` is set, and served on a
//! listener of their own.

use std::time::Duration;

use reqwest::{Method, StatusCode};

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TestServer};

/// A server that keeps metrics and serves them at a free local port.
async fn server_with_metrics() -> TestServer {
    Setup::native().metrics_at("127.0.0.1:0").start().await
}

/// What a `GET` of `path` on the metrics listener answers: the status, the
/// `Content-Type` and the body.
async fn scrape_path(server: &TestServer, path: &str) -> (StatusCode, String, String) {
    let address = server.metrics_address.expect("the server keeps metrics");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("an HTTP client")
        .get(format!("http://{address}{path}"))
        .send()
        .await
        .expect("the metrics listener answers");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.text().await.expect("a text body");
    (status, content_type, body)
}

/// The metrics as the listener serves them.
pub(crate) async fn scrape(server: &TestServer) -> String {
    let (status, content_type, body) = scrape_path(server, "/metrics").await;
    assert_eq!(status, StatusCode::OK, "a scrape is answered: {body}");
    assert_eq!(
        content_type, "text/plain; version=0.0.4",
        "the Prometheus text format"
    );
    body
}

#[tokio::test]
async fn no_metrics_are_kept_and_nothing_listens_unless_an_address_is_set() {
    let server = Setup::native().start().await;
    assert_eq!(server.metrics_address, None, "no metrics listener");
    assert!(!server.app.metrics().is_kept(), "no metrics are kept");
    server.teardown().await;
}

#[tokio::test]
async fn the_metrics_are_served_on_a_listener_of_their_own() {
    let server = server_with_metrics().await;
    let body = scrape(&server).await;
    let version = env!("CARGO_PKG_VERSION");
    let commit = option_env!("FVOCI_BUILD_HASH").unwrap_or_default();
    assert!(
        body.contains(&format!(
            "fvoci_build_info{{commit=\"{commit}\",version=\"{version}\"}} 1"
        )),
        "the build information: {body}"
    );
    #[cfg(target_os = "linux")]
    for name in ["process_resident_memory_bytes", "process_open_fds"] {
        assert!(
            body.contains(&format!("\n{name} ")),
            "the process metric {name}: {body}"
        );
    }
    // Nothing but the metrics: no route of the API or the web app.
    for path in ["/", "/api/v1/ping", "/ws", "/static/main.js", "/metrics/"] {
        let (status, content_type, body) = scrape_path(&server, path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} on the metrics listener"
        );
        // Refused as the API refuses: a problem, whose type has its page here too.
        assert_eq!(content_type, "application/problem+json", "{path}");
        let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem");
        assert_eq!(
            problem,
            serde_json::json!({
                "type": "/problems/not-found", "title": "Not found", "status": 404,
                "detail": "route not found",
            }),
            "{path}: no request is identified, as none is logged here"
        );
    }
    let (status, content_type, body) = scrape_path(&server, "/problems/not-found").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the page a refusal points at: {body}"
    );
    assert_eq!(content_type, "text/plain; charset=utf-8");
    assert!(
        body.starts_with("Not found\n\ntype: /problems/not-found\n"),
        "{body}"
    );
    let address = server.metrics_address.expect("the server keeps metrics");
    let posted = reqwest::Client::new()
        .post(format!("http://{address}/metrics"))
        .send()
        .await
        .expect("the metrics listener answers");
    assert_eq!(posted.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        posted
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
        "a method the path does not have"
    );
    server.teardown().await;
}

#[tokio::test]
async fn the_api_listener_does_not_serve_the_metrics() {
    let server = server_with_metrics().await;
    // A path of the web app's: its page, not the metrics.
    let response = Client::send(server.client.raw_request(Method::GET, "/metrics")).await;
    let body = String::from_utf8_lossy(&response.body);
    assert!(!body.contains("fvoci_build_info"), "no metrics: {body}");
    assert!(body.contains("test web app"), "the web app's page: {body}");
    let response = server.client.get("/metrics").await;
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "no metrics under the API"
    );
    server.teardown().await;
}

#[tokio::test]
async fn a_metrics_address_that_is_taken_keeps_the_server_from_starting() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = taken.local_addr().expect("an address").to_string();
    let error = Setup::native()
        .metrics_at(&address)
        .try_start()
        .await
        .err()
        .expect("the server does not start");
    let error = format!("{error:#}");
    assert!(
        error.contains("metrics_address"),
        "the setting is named: {error}"
    );
    drop(taken);
}

/// The value of the sample of `name` whose labels are exactly `labels`, given in the
/// order of their names, as the text format writes them.
pub(crate) fn sample(body: &str, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    let labels: Vec<String> = labels
        .iter()
        .map(|(label, value)| format!("{label}=\"{value}\""))
        .collect();
    let series = if labels.is_empty() {
        name.to_owned()
    } else {
        format!("{name}{{{}}}", labels.join(","))
    };
    body.lines().find_map(|line| {
        let value = line.strip_prefix(&series)?.strip_prefix(' ')?;
        value.parse().ok()
    })
}

/// The lines of the samples of `name`.
fn series<'a>(body: &'a str, name: &str) -> Vec<&'a str> {
    body.lines()
        .filter(|line| line.starts_with(name) && line[name.len()..].starts_with(['{', ' ']))
        .collect()
}

/// How many requests the histogram counted with these labels.
fn requests(body: &str, method: &str, route: &str, status: &str) -> Option<f64> {
    sample(
        body,
        "fvoci_http_request_duration_seconds_count",
        &[("method", method), ("route", route), ("status", status)],
    )
}

#[tokio::test]
async fn requests_are_counted_by_method_route_template_and_status() {
    let server = server_with_metrics().await;
    for _ in 0..3 {
        let response = server.client.get("/ping").await;
        assert_eq!(response.status, StatusCode::OK);
    }
    let response = Client::send(server.client.raw_request(Method::GET, "/static/main.js")).await;
    assert_eq!(response.status, StatusCode::OK);
    let body = scrape(&server).await;
    assert_eq!(
        requests(&body, "GET", "/api/v1/ping", "200"),
        Some(3.0),
        "{body}"
    );
    assert_eq!(
        requests(&body, "GET", "/static/{*path}", "200"),
        Some(1.0),
        "{body}"
    );
    assert_eq!(
        sample(&body, "fvoci_http_requests_in_flight", &[]),
        Some(0.0),
        "nothing is being answered: {body}"
    );
    server.teardown().await;
}

#[tokio::test]
async fn paths_with_identifiers_are_counted_under_their_template() {
    let server = server_with_metrics().await;
    let ids: Vec<String> = (0..3).map(|_| fvoci_model::new_id()).collect();
    for id in &ids {
        let response = server
            .client
            .get(&format!("{TEST_WS}/containers/{id}"))
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "no session");
    }
    let body = scrape(&server).await;
    assert_eq!(
        requests(
            &body,
            "GET",
            "/api/v1/w/{workspaceId}/containers/{containerId}",
            "401"
        ),
        Some(3.0),
        "one series for the three boards: {body}"
    );
    for id in &ids {
        assert!(
            !body.contains(id.as_str()),
            "no identifier in a label: {body}"
        );
    }
    server.teardown().await;
}

#[tokio::test]
async fn paths_and_methods_that_are_no_route_do_not_make_labels() {
    let server = server_with_metrics().await;
    let ask_nonsense = async |count: usize| {
        let mut asked = Vec::new();
        for n in 0..count {
            let word = fvoci_model::new_id();
            let path = match n % 3 {
                0 => format!("/api/v1/{word}"),
                1 => format!("/{word}/{n}"),
                _ => format!("/api/v2/containers/{word}"),
            };
            let method = if n % 2 == 0 {
                Method::GET
            } else {
                Method::from_bytes(format!("X{n}").as_bytes()).expect("a method")
            };
            Client::send(server.client.raw_request(method, &path)).await;
            asked.push(word);
        }
        asked
    };
    let name = "fvoci_http_request_duration_seconds_count";
    let mut asked = ask_nonsense(6).await;
    let before = scrape(&server).await;
    asked.extend(ask_nonsense(60).await);
    let after = scrape(&server).await;
    assert_eq!(
        series(&before, name).len(),
        series(&after, name).len(),
        "ten times the nonsense makes no new series: {after}"
    );
    for word in &asked {
        assert!(
            !after.contains(word.as_str()),
            "nothing asked is a label: {after}"
        );
    }
    assert!(
        series(&after, name)
            .iter()
            .any(|line| line.contains("method=\"other\"") && line.contains("route=\"unmatched\"")),
        "an unknown method on an unknown path: {after}"
    );
    server.teardown().await;
}

#[tokio::test]
async fn the_base_path_is_not_part_of_the_route() {
    let server = Setup::native()
        .base_path(&format!("{TEST_WS}/boards"))
        .metrics_at("127.0.0.1:0")
        .start()
        .await;
    let response = server.client.get("/ping").await;
    assert_eq!(response.status, StatusCode::OK);
    let body = scrape(&server).await;
    assert_eq!(
        requests(&body, "GET", "/api/v1/ping", "200"),
        Some(1.0),
        "{body}"
    );
    server.teardown().await;
}

/// Scrapes until `holds` is true of the metrics, for ten seconds at most; returns the
/// last scrape.
pub(crate) async fn scrape_until(server: &TestServer, holds: impl Fn(&str) -> bool) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body = scrape(server).await;
        if holds(&body) || tokio::time::Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A websocket connection's next text message, skipping pings; `None` if none comes
/// within `wait` or the connection ends.
async fn next_text(socket: &mut crate::common::WebSocket, wait: Duration) -> Option<String> {
    use futures_util::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;
    loop {
        match tokio::time::timeout(wait, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => return Some(text.to_string()),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            _ => return None,
        }
    }
}

#[tokio::test]
async fn websocket_connections_messages_and_disconnections_are_counted() {
    use futures_util::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message;

    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let user = server.client.me().await;
    let mut socket = server.client.connect_websocket().await;
    let mut other = server.client2.connect_websocket().await;
    let connections = |body: &str| sample(body, "fvoci_websocket_connections", &[]);
    let body = scrape_until(&server, |body| connections(body) == Some(2.0)).await;
    assert_eq!(connections(&body), Some(2.0), "two open: {body}");

    // Messages are counted as they are queued, one per connection they go to.
    socket
        .send(Message::Text(
            serde_json::json!({"type": "subscribeWorkspace", "workspaceId": crate::common::ROOT_WORKSPACE_ID})
                .to_string()
                .into(),
        ))
        .await
        .expect("the command is sent");
    let sub_ack = next_text(&mut socket, Duration::from_secs(5))
        .await
        .expect("workspaceSubscribed ACK");
    assert!(
        sub_ack.contains("workspaceSubscribed"),
        "expected workspaceSubscribed ACK: {sub_ack}"
    );
    let probe = fvoci_model::Category {
        id: fvoci_model::new_id(),
        name: "probe".to_owned(),
        user_id: user.id.clone(),
        workspace_id: crate::common::ROOT_WORKSPACE_ID.to_owned(),
        ..fvoci_model::Category::default()
    };
    server.app.ws().broadcast_category_change(&probe).await;
    assert!(
        next_text(&mut socket, Duration::from_secs(5))
            .await
            .is_some(),
        "the socket received categoryChanged"
    );
    let mut received = 1_u32;
    while next_text(&mut socket, Duration::from_millis(200))
        .await
        .is_some()
    {
        received += 1;
    }
    assert!(received > 0, "the socket was subscribed");
    let sent = |body: &str| {
        sample(
            body,
            "fvoci_websocket_messages_queued_total",
            &[("type", "categoryChanged")],
        )
    };
    let body = scrape(&server).await;
    assert_eq!(sent(&body), Some(f64::from(received)), "{body}");

    // The client closes one; the other's session ends.
    other.close(None).await.expect("the close is sent");
    server.client.post_empty("/logout").await.check_no_content();
    assert_eq!(
        next_text(&mut socket, Duration::from_secs(5)).await,
        None,
        "closed"
    );
    let disconnections = |body: &str, reason: &str| {
        sample(
            body,
            "fvoci_websocket_disconnections_total",
            &[("reason", reason)],
        )
    };
    let body = scrape_until(&server, |body| connections(body) == Some(0.0)).await;
    assert_eq!(connections(&body), Some(0.0), "none open: {body}");
    assert_eq!(disconnections(&body, "client_closed"), Some(1.0), "{body}");
    assert_eq!(disconnections(&body, "session_ended"), Some(1.0), "{body}");
    server.teardown().await;
}

/// The open uploads and their bytes, as scrapes find them once they are counted anew:
/// a count is kept five seconds, so this scrapes until both are `want`, fifteen seconds
/// at most (two counts, one of which may fail after two seconds, and slack).
async fn open_uploads_become(server: &TestServer, want: (f64, f64)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let body = scrape(server).await;
        let found = (
            sample(&body, "fvoci_uploads_open", &[]),
            sample(&body, "fvoci_uploads_open_bytes", &[]),
        );
        if found == (Some(want.0), Some(want.1)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the open uploads are {found:?}, not {want:?}: {body}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn the_pool_and_the_open_uploads_are_measured() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let counted = tokio::time::Instant::now();
    let body = scrape(&server).await;
    let max = sample(&body, "fvoci_db_pool_max_connections", &[]).expect("the pool's size");
    let open = sample(&body, "fvoci_db_pool_connections", &[]).expect("its connections");
    let idle = sample(&body, "fvoci_db_pool_idle_connections", &[]).expect("its idle ones");
    assert!(max >= 1.0 && open <= max && idle <= open, "{body}");
    assert_eq!(
        sample(&body, "fvoci_uploads_open", &[]),
        Some(0.0),
        "{body}"
    );

    let board = server
        .create_board(
            crate::common::TEST_WORKSPACE_ID,
            fvoci_model::ContainerVisibility::Open,
        )
        .await;
    let mut uploads = Vec::new();
    for size in [10, 20] {
        let opened = server
            .client
            .post(
                &format!("{TEST_WS}/containers/{}/uploads", board.id),
                &serde_json::json!({"name": "a.txt", "size": size}),
            )
            .await;
        opened.check_created();
        uploads.push(
            opened.value()["uploadId"]
                .as_str()
                .expect("an id")
                .to_owned(),
        );
    }
    // However often a scrape comes, the database is asked at most once in five
    // seconds: a scrape soon after the count keeps it.
    let body = scrape(&server).await;
    if counted.elapsed() < Duration::from_secs(4) {
        assert_eq!(
            sample(&body, "fvoci_uploads_open", &[]),
            Some(0.0),
            "the count of a moment ago: {body}"
        );
        assert_eq!(
            sample(&body, "fvoci_uploads_read_failures_total", &[]),
            Some(0.0),
            "kept, not failed: {body}"
        );
    }
    open_uploads_become(&server, (2.0, 30.0)).await;
    server
        .client
        .delete(&format!("{TEST_WS}/uploads/{}", uploads[0]))
        .await
        .check_no_content();
    open_uploads_become(&server, (1.0, 20.0)).await;

    server.teardown().await;
}

#[tokio::test]
async fn the_archives_that_hold_their_place_are_counted() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let board = server
        .create_board(
            crate::common::TEST_WORKSPACE_ID,
            fvoci_model::ContainerVisibility::Open,
        )
        .await;
    let export = server
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    assert_eq!(export.status, StatusCode::OK);
    let imported = crate::export::import(
        &server.client,
        crate::common::TEST_WORKSPACE_ID,
        &export.body,
    )
    .await;
    assert_eq!(imported.status, StatusCode::OK, "the archive is imported");
    let archives =
        |body: &str, work: &str| sample(body, "fvoci_archives_in_progress", &[("work", work)]);
    // The export's place is given back once its file is removed, in a task after the
    // download.
    let body = scrape_until(&server, |body| archives(body, "export") == Some(0.0)).await;
    assert_eq!(
        archives(&body, "export"),
        Some(0.0),
        "the export is done: {body}"
    );
    assert_eq!(
        archives(&body, "import"),
        Some(0.0),
        "the import is done: {body}"
    );
    // What is counted is what the limits count: the places held.
    let user = server.client.me().await;
    let export = server
        .app
        .start_export(&user.id)
        .expect("a place for an export");
    let import = server
        .app
        .start_import(&user.id)
        .expect("a place for an import");
    let held = scrape(&server).await;
    assert_eq!(archives(&held, "export"), Some(1.0), "{held}");
    assert_eq!(archives(&held, "import"), Some(1.0), "{held}");
    drop((export, import));
    assert_eq!(
        archives(&scrape(&server).await, "export"),
        Some(0.0),
        "given back"
    );
    let export_route = "/api/v1/w/{workspaceId}/containers/{containerId}/archive/export";
    let import_route = "/api/v1/w/{workspaceId}/archive/import";
    assert_eq!(
        requests(&body, "GET", export_route, "200"),
        Some(1.0),
        "{body}"
    );
    assert_eq!(
        requests(&body, "POST", import_route, "200"),
        Some(1.0),
        "{body}"
    );
    server.teardown().await;
}

async fn export_and_import_container(
    client: &crate::common::Client,
    workspace_id: &str,
    container_id: &str,
) {
    let export = client
        .get(&format!(
            "/w/{workspace_id}/containers/{container_id}/archive/export"
        ))
        .await;
    export.check_ok();
    crate::export::import(client, workspace_id, &export.body)
        .await
        .check_ok();
}

#[tokio::test]
async fn the_boards_wikis_and_diaries_copied_whole_are_counted_by_work_and_kind() {
    let server = server_with_metrics().await.init_basic().await;
    let client = &server.client;
    let copied = |body: &str, work: &str, kind: &str| {
        sample(
            body,
            "fvoci_containers_copied_total",
            &[("kind", kind), ("work", work)],
        )
    };
    let before = scrape(&server).await;
    for work in ["export", "import", "duplicate", "welcome"] {
        for kind in ["board", "wiki", "diary", "course"] {
            assert_eq!(
                copied(&before, work, kind),
                Some(0.0),
                "{work} {kind}: {before}"
            );
        }
    }
    let board = server
        .create_board(
            crate::common::TEST_WORKSPACE_ID,
            fvoci_model::ContainerVisibility::Open,
        )
        .await;
    let wiki = crate::wiki_copies::wiki(client, "Counted").await;
    for (id, times) in [(&board.id, 1), (&wiki, 2)] {
        for _ in 0..times {
            export_and_import_container(client, crate::common::TEST_WORKSPACE_ID, id).await;
        }
        client
            .post(
                &format!("{TEST_WS}/containers/{id}/duplicate"),
                &serde_json::json!({}),
            )
            .await
            .check_created();
    }
    let own = crate::own_space::own_workspace(client).await.id;
    let diary = client
        .post(&format!("/w/{own}/diaries"), &serde_json::json!({}))
        .await;
    diary.check_created();
    let diary = diary.value()["id"].as_str().expect("its id").to_owned();
    export_and_import_container(client, &own, &diary).await;
    let course = client
        .post(
            &format!("{TEST_WS}/courses"),
            &serde_json::json!({"visibility": "private", "title": "Counted"}),
        )
        .await;
    course.check_created();
    let course = course.value()["id"].as_str().expect("its id").to_owned();
    export_and_import_container(client, crate::common::TEST_WORKSPACE_ID, &course).await;
    // The welcome board of a new account is counted apart from the copies people make.
    client
        .post_empty(&format!("{TEST_WS}/onboard"))
        .await
        .check_ok();
    let body = scrape(&server).await;
    let counted = [
        ("welcome", "board", 1.0),
        ("welcome", "wiki", 0.0),
        ("export", "board", 1.0),
        ("import", "board", 1.0),
        ("duplicate", "board", 1.0),
        ("export", "wiki", 2.0),
        ("import", "wiki", 2.0),
        ("duplicate", "wiki", 1.0),
        ("welcome", "diary", 0.0),
        ("export", "diary", 1.0),
        ("import", "diary", 1.0),
        ("duplicate", "diary", 0.0),
        ("welcome", "course", 0.0),
        ("export", "course", 1.0),
        ("import", "course", 1.0),
        ("duplicate", "course", 0.0),
    ];
    for (work, kind, times) in counted {
        assert_eq!(
            copied(&body, work, kind),
            Some(times),
            "{work} {kind}: {body}"
        );
    }
    // Four values of `kind`, four of `work`: nothing else is counted.
    let series = body
        .lines()
        .filter(|line| line.starts_with("fvoci_containers_copied_total{"))
        .count();
    assert_eq!(series, 16, "{body}");
    server.teardown().await;
}

#[tokio::test]
async fn the_background_jobs_are_counted_from_their_first_run() {
    let server = server_with_metrics().await;
    let job = |body: &str, name: &str, job: &str| sample(body, name, &[("job", job)]);
    // Both run once as the server starts: the uploads before it serves, the trash
    // right after.
    let body = scrape_until(&server, |body| {
        job(body, "fvoci_job_runs_total", "purge_trash") == Some(1.0)
    })
    .await;
    for name in ["purge_trash", "clean_up_uploads"] {
        assert_eq!(
            job(&body, "fvoci_job_runs_total", name),
            Some(1.0),
            "{name}: {body}"
        );
        assert_eq!(
            job(&body, "fvoci_job_failures_total", name),
            Some(0.0),
            "{name}: {body}"
        );
        let last = job(&body, "fvoci_job_last_run_timestamp_seconds", name).unwrap_or_default();
        assert!(last > 1.7e9, "{name} ran just now: {body}");
        let success = job(&body, "fvoci_job_last_success_timestamp_seconds", name);
        assert_eq!(success, Some(last), "{name} did all its work: {body}");
    }
    server.teardown().await;
}

#[tokio::test]
async fn attempts_the_login_limits_refuse_are_counted_by_limit() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| config.login_pair_free_failures = 1)
        .start()
        .await;
    let attempt = async || {
        server
            .anonymous()
            .post(
                "/login",
                &serde_json::json!({"type": "normal", "username": "nobody", "password": "a guess"}),
            )
            .await
            .status
    };
    assert_eq!(attempt().await, StatusCode::UNAUTHORIZED, "free");
    assert_eq!(
        attempt().await,
        StatusCode::UNAUTHORIZED,
        "the address waits after it"
    );
    assert_eq!(
        attempt().await,
        StatusCode::TOO_MANY_REQUESTS,
        "made to wait"
    );
    let refusals =
        |body: &str, limit: &str| sample(body, "fvoci_login_refusals_total", &[("limit", limit)]);
    let body = scrape(&server).await;
    assert_eq!(refusals(&body, "pair_delay"), Some(1.0), "{body}");
    assert_eq!(
        sample(&body, "fvoci_logins_total", &[("outcome", "limited")]),
        Some(1.0),
        "and the login is counted as limited: {body}"
    );
    for limit in ["account_lock", "address_busy", "address_rate"] {
        assert_eq!(refusals(&body, limit), Some(0.0), "{limit}: {body}");
    }
    server.teardown().await;

    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| config.login_address_attempts_per_minute = 1)
        .start()
        .await;
    let attempt = async || {
        server
            .anonymous()
            .post(
                "/login",
                &serde_json::json!({"type": "normal", "username": "nobody", "password": "a guess"}),
            )
            .await
            .status
    };
    assert_eq!(
        attempt().await,
        StatusCode::UNAUTHORIZED,
        "the first of the minute"
    );
    assert_eq!(
        attempt().await,
        StatusCode::TOO_MANY_REQUESTS,
        "one a minute"
    );
    let body = scrape(&server).await;
    assert_eq!(refusals(&body, "address_rate"), Some(1.0), "{body}");
    assert_eq!(refusals(&body, "pair_delay"), Some(0.0), "{body}");
    server.teardown().await;
}

#[tokio::test]
async fn a_local_only_server_does_not_start_with_metrics_on_every_interface() {
    for every in ["0.0.0.0:0", "[::]:0"] {
        let error = Setup::native()
            .metrics_at(every)
            .configure(|config| config.listen_loopback_only = true)
            .try_start()
            .await
            .err()
            .expect("the server does not start");
        let error = format!("{error:#}");
        assert!(
            error.contains("metrics_address") && error.contains("listen_loopback_only"),
            "{every}: {error}"
        );
    }
    let server = Setup::native()
        .metrics_at("localhost:0")
        .configure(|config| config.listen_loopback_only = true)
        .start()
        .await;
    let address = server.metrics_address.expect("the server keeps metrics");
    assert!(address.ip().is_loopback(), "{address}");
    scrape(&server).await;
    server.teardown().await;
}

#[tokio::test]
async fn with_a_token_set_a_scrape_must_send_it() {
    const TOKEN: &str = "a-token-for-the-scrapes-of-32-chars";
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| TOKEN.clone_into(&mut config.metrics_token))
        .start()
        .await;
    let address = server.metrics_address.expect("the server keeps metrics");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("an HTTP client");
    let get = |authorization: Option<&str>| {
        let mut request = client.get(format!("http://{address}/metrics"));
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        request.send()
    };
    for authorization in [None, Some("Bearer not-the-token")] {
        let response = get(authorization).await.expect("an answer");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{authorization:?}"
        );
        let challenge = if authorization.is_some() {
            "Bearer realm=\"metrics\", error=\"invalid_token\""
        } else {
            "Bearer realm=\"metrics\""
        };
        assert_eq!(
            response
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok()),
            Some(challenge)
        );
        let body = response.text().await.expect("a body");
        let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem");
        assert_eq!(problem["type"], "/problems/unauthorized", "{body}");
        assert!(!body.contains(TOKEN) && !body.contains("fvoci_"), "{body}");
    }
    let response = get(Some(&format!("Bearer {TOKEN}")))
        .await
        .expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("a body");
    assert!(body.contains("fvoci_build_info"), "{body}");
    server.teardown().await;
}
