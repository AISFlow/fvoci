#![cfg(feature = "db-tests")]

use fvoci_server::search::meili::{
    delete_meili_by_filter, ensure_meili_index, ensure_scoped_meili_key, meili_eq, search_meili,
    search_source_id, upsert_meili_sources, MeiliConfig, MeiliSearchInput, MeiliSearchScope,
    SearchSource, SearchSourceKind,
};
use fvoci_server::search::text::index_document_text;
use uuid::Uuid;

fn test_config() -> MeiliConfig {
    let url = std::env::var("FVOCI_MEILI_URL").expect("FVOCI_MEILI_URL");
    let key = std::env::var("FVOCI_MEILI_KEY").expect("FVOCI_MEILI_KEY");
    let index_uid = format!("fvoci_{}", Uuid::now_v7().simple());
    MeiliConfig::new(url, key, index_uid)
}

fn document_source(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    document_id: Uuid,
    title: &str,
    body: &str,
) -> SearchSource {
    let text = index_document_text(title, body, "");
    SearchSource {
        id: search_source_id(SearchSourceKind::Document, &document_id.to_string(), None),
        kind: SearchSourceKind::Document,
        workspace_id: workspace_id.to_string(),
        project_id: project_id.map(|id| id.to_string()),
        document_id: Some(document_id.to_string()),
        task_id: None,
        comment_id: None,
        attachment_id: None,
        chunk_no: None,
        title: text.title,
        body: text.body,
        chosung: text.chosung,
        stem: text.stem,
        updated_at: 1,
        embedding: None,
    }
}

async fn fetch_settings(config: &MeiliConfig) -> serde_json::Value {
    let url = format!("{}/indexes/{}/settings", config.url, config.index_uid);
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(config.api_key())
        .send()
        .await
        .expect("settings request");
    assert_eq!(resp.status().as_u16(), 200, "settings HTTP");
    resp.json().await.expect("settings json")
}

/// Ensures index/settings, upserts, searches with workspace/project/wiki scopes,
/// deletes by filter, and rejects filter-builder injection.
#[tokio::test]
async fn ensure_upsert_search_scope_and_delete_by_filter() {
    assert!(meili_eq("workspaceId", "a OR 1=1").is_err());
    assert!(meili_eq(
        "workspaceId",
        "11111111-1111-4111-8111-111111111111\" OR kind = \"document"
    )
    .is_err());
    assert!(meili_eq("kind); DROP", "document").is_err());
    assert_eq!(
        meili_eq("kind", "document").unwrap(),
        r#"kind = "document""#
    );

    let config = test_config();
    ensure_meili_index(&config)
        .await
        .unwrap_or_else(|e| panic!("ensure index: {e}"));
    let settings = fetch_settings(&config).await;
    assert_eq!(
        settings["searchableAttributes"],
        serde_json::json!(["title", "body", "chosung", "stem"])
    );
    let filterable = settings["filterableAttributes"]
        .as_array()
        .expect("filterableAttributes");
    for attr in [
        "kind",
        "workspaceId",
        "projectId",
        "documentId",
        "resourceKey",
    ] {
        assert!(
            filterable.iter().any(|v| v.as_str() == Some(attr)),
            "missing filterable {attr}: {settings}"
        );
    }
    assert_eq!(
        settings["embedders"]["attachments"]["source"],
        "userProvided"
    );
    assert_eq!(settings["embedders"]["attachments"]["dimensions"], 1536);

    let ws_a = Uuid::now_v7();
    let ws_b = Uuid::now_v7();
    let project_a = Uuid::now_v7();
    let project_b = Uuid::now_v7();
    let doc_a = Uuid::now_v7();
    let doc_wiki = Uuid::now_v7();
    let doc_b = Uuid::now_v7();
    let token = "qvoxsearchtoken";

    upsert_meili_sources(
        &config,
        &[
            document_source(ws_a, Some(project_a), doc_a, token, "project body"),
            document_source(ws_a, None, doc_wiki, token, "wiki body"),
            document_source(ws_b, Some(project_b), doc_b, token, "other workspace"),
        ],
    )
    .await
    .unwrap_or_else(|e| panic!("upsert: {e}"));

    let injected = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: vec!["a OR 1=1".into()],
                include_wiki: false,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await;
    assert!(injected.is_err(), "filter builder must reject injection");

    let project_page = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: vec![project_a.to_string()],
                include_wiki: false,
                wiki_document_ids: Vec::new(),
            }],
            kind: Some(SearchSourceKind::Document),
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("project search: {e}"));
    let project_ids: Vec<_> = project_page.hits.iter().map(|h| h.id.as_str()).collect();
    let expected_a = search_source_id(SearchSourceKind::Document, &doc_a.to_string(), None);
    assert_eq!(project_ids, vec![expected_a.as_str()]);

    let wiki_page = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: Vec::new(),
                include_wiki: true,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("wiki search: {e}"));
    assert_eq!(wiki_page.hits.len(), 1);
    assert_eq!(
        wiki_page.hits[0].document_id.as_deref(),
        Some(doc_wiki.to_string()).as_deref()
    );

    let guest_page = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: Vec::new(),
                include_wiki: false,
                wiki_document_ids: vec![doc_wiki.to_string()],
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("guest wiki search: {e}"));
    assert_eq!(guest_page.hits.len(), 1);
    assert_eq!(
        guest_page.hits[0].id,
        search_source_id(SearchSourceKind::Document, &doc_wiki.to_string(), None)
    );

    let both_page = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: vec![project_a.to_string()],
                include_wiki: true,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("combined search: {e}"));
    let mut both_ids: Vec<_> = both_page
        .hits
        .iter()
        .map(|h| h.resource_id.clone())
        .collect();
    both_ids.sort();
    let mut expected = vec![doc_a.to_string(), doc_wiki.to_string()];
    expected.sort();
    assert_eq!(both_ids, expected);
    assert!(both_page
        .hits
        .iter()
        .all(|h| h.workspace_id == ws_a.to_string()));
    assert!(!both_page
        .hits
        .iter()
        .any(|h| h.resource_id == doc_b.to_string()));

    delete_meili_by_filter(
        &config,
        &meili_eq("workspaceId", &ws_a.to_string()).expect("ws filter"),
    )
    .await
    .unwrap_or_else(|e| panic!("delete by filter: {e}"));

    let after_delete = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_a.to_string(),
                project_ids: vec![project_a.to_string()],
                include_wiki: true,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("search after delete: {e}"));
    assert!(after_delete.hits.is_empty());

    let other = search_meili(
        &config,
        &MeiliSearchInput {
            q: token.into(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: ws_b.to_string(),
                project_ids: vec![project_b.to_string()],
                include_wiki: false,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("other workspace search: {e}"));
    assert_eq!(other.hits.len(), 1);
    assert_eq!(other.hits[0].resource_id, doc_b.to_string());
}

async fn keys_status(url: &str, key: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{}/keys", url.trim_end_matches('/')))
        .bearer_auth(key)
        .send()
        .await
        .expect("GET /keys")
        .status()
        .as_u16()
}

#[tokio::test]
async fn scoped_key_is_index_only_reused_and_replaces_a_master_key_file() {
    // CI passes the master key as FVOCI_MEILI_KEY; the server must only ever get
    // the scoped key that this path writes.
    let url = std::env::var("FVOCI_MEILI_URL").expect("FVOCI_MEILI_URL");
    let master = std::env::var("FVOCI_MEILI_KEY").expect("FVOCI_MEILI_KEY");
    let index_uid = format!("fvoci_{}", Uuid::now_v7().simple());
    let dir = std::env::temp_dir().join(format!("fvoci-meili-key-{}", Uuid::now_v7()));
    let dest = dir.join("key");

    ensure_scoped_meili_key(&url, &master, &index_uid, &dest)
        .await
        .expect("create scoped key");
    let scoped = std::fs::read_to_string(&dest).expect("key file");
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(&dest).expect("key meta").permissions(),
    );
    assert_eq!(mode & 0o777, 0o600, "key file must be private");
    assert_ne!(scoped.trim(), master.trim());
    assert_eq!(
        keys_status(&url, scoped.trim()).await,
        403,
        "scoped key cannot manage keys"
    );
    ensure_meili_index(&MeiliConfig::new(
        url.clone(),
        scoped.trim().to_string(),
        index_uid.clone(),
    ))
    .await
    .expect("scoped key can ensure its own index");

    ensure_scoped_meili_key(&url, &master, &index_uid, &dest)
        .await
        .expect("reuse scoped key");
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        scoped,
        "valid scoped key is reused"
    );

    // A master key placed in the file must not be kept.
    std::fs::write(&dest, master.trim()).unwrap();
    ensure_scoped_meili_key(&url, &master, &index_uid, &dest)
        .await
        .expect("replace master key file");
    let replaced = std::fs::read_to_string(&dest).unwrap();
    assert_ne!(
        replaced.trim(),
        master.trim(),
        "master key in the file must be replaced"
    );
    assert_eq!(keys_status(&url, replaced.trim()).await, 403);

    let _ = std::fs::remove_dir_all(&dir);
}
