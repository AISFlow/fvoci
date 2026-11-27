//! End-to-end integration, adversarial security, SSRF, and atomicity tests for external
//! imports (Trello JSON and Notion Markdown/CSV ZIP) across both SQLite and PostgreSQL.

use std::collections::HashSet;
use std::io::{Cursor, Write as _};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fvoci_model::{
    Block, BlockType, Container, ContainerKind, PAGE_ORDER_FIELD, PageNode, WorkspaceRole, new_id,
};
use fvoci_server::app::{BundleLimits, FileToUpload, ImportFormat, ImportWarningCode};
use fvoci_server::error::ErrorKind;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use zip::write::SimpleFileOptions;

use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::export::put_part;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fvoci-import-api-{}", new_id()));
        std::fs::create_dir_all(&path).expect("create integration test temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Spawns a local loopback TCP listener that counts every incoming connection to prove
/// external URLs in Trello/Notion imports never trigger outbound network requests (SSRF).
async fn spawn_ssrf_trap() -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("bind loopback SSRF trap");
    let addr = listener.local_addr().expect("trap local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let task = tokio::spawn(async move {
        while let Ok((_stream, _peer)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    (addr, hits, task)
}

/// Builds a ZIP archive in memory from `(entry_name, content, options)` tuples.
fn build_zip(entries: &[(&str, &[u8], SimpleFileOptions)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content, opts) in entries {
        writer.start_file(*name, *opts).expect("start zip entry");
        writer.write_all(content).expect("write zip entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

/// Recursively counts files stored under the server's local file storage root, ignoring `.tmp`.
fn count_stored_files(root: &Path) -> usize {
    fn walk(dir: &Path) -> usize {
        let Ok(read) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut total = 0;
        for entry in read.flatten() {
            if entry.file_name() == ".tmp" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                total += walk(&path);
            } else if path.is_file() {
                total += 1;
            }
        }
        total
    }
    walk(root)
}

/// Asserts that `TEST_WORKSPACE_ID` has zero boards, zero wikis, and zero files in storage.
async fn assert_nothing_stored(ts: &TestServer, context: &str) {
    let boards: Vec<Container> = ts.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let wikis: Vec<Container> = ts.client.get(&format!("{TEST_WS}/wikis")).await.ok_json();
    let stored_files = count_stored_files(Path::new(&ts.app.config().file_storage_path));
    assert!(
        boards.is_empty(),
        "{context}: expected 0 boards, found {}",
        boards.len()
    );
    assert!(
        wikis.is_empty(),
        "{context}: expected 0 wikis, found {}",
        wikis.len()
    );
    assert_eq!(
        stored_files, 0,
        "{context}: expected 0 stored files in file backend"
    );
}

fn find_nodes_by_type<'a>(value: &'a Value, node_type: &str, out: &mut Vec<&'a Value>) {
    if let Some(obj) = value.as_object() {
        if obj.get("type").and_then(Value::as_str) == Some(node_type) {
            out.push(value);
        }
        if let Some(Value::Array(children)) = obj.get("content") {
            for child in children {
                find_nodes_by_type(child, node_type, out);
            }
        }
    }
}

fn build_trello_fixture(trap_img_url: &str, trap_att_url: &str) -> String {
    let trello_json = json!({
        "id": "board-trello-1",
        "name": "Product Launch",
        "desc": "Quarterly release checklist and tasks.",
        "lists": [
            {"id": "list-todo", "name": "To Do", "closed": false, "pos": 100.0},
            {"id": "list-done", "name": "Done", "closed": false, "pos": 200.0},
            {"id": "list-arch", "name": "Old Archive", "closed": true, "pos": 300.0}
        ],
        "labels": [
            {"id": "lbl-bug", "name": "Bug", "color": "red"},
            {"id": "lbl-green", "name": "", "color": "green"}
        ],
        "cards": [
            {
                "id": "card-1",
                "name": "Ship v1 release",
                "desc": "## Notes\n\nShip **carefully**.\n\n![Banner](TRAP_IMG)\n\n| Area | Owner |\n| --- | --- |\n| API | Ready |",
                "idList": "list-todo",
                "idLabels": ["lbl-bug", "lbl-green"],
                "start": "2026-06-01T00:00:00.000Z",
                "due": "2026-06-15T12:00:00.000Z",
                "pos": 10.0,
                "closed": false,
                "idChecklists": ["chk-1"],
                "attachments": [
                    {"id": "att-1", "name": "Secret Spec", "url": trap_att_url},
                    {"id": "att-2", "name": "Public Docs", "url": "https://example.com/docs"}
                ]
            },
            {
                "id": "card-2",
                "name": "Prepare announcement",
                "desc": "Draft blog post.",
                "idList": "list-done",
                "idLabels": [],
                "pos": 20.0,
                "closed": false
            },
            {"id": "card-closed", "name": "Closed card", "idList": "list-todo", "closed": true},
            {"id": "card-in-closed-list", "name": "Card in archived list", "idList": "list-arch", "closed": false}
        ],
        "checklists": [
            {
                "id": "chk-1",
                "name": "Pre-flight",
                "idCard": "card-1",
                "pos": 1.0,
                "checkItems": [
                    {"id": "ci-1", "name": "Run migrations", "state": "complete", "pos": 1.0},
                    {"id": "ci-2", "name": "Tag release", "state": "incomplete", "pos": 2.0}
                ]
            }
        ],
        "actions": [
            {
                "id": "act-1",
                "type": "commentCard",
                "date": "2026-06-02T08:30:00.000Z",
                "data": {"text": "Looks **good** to ship.", "card": {"id": "card-1"}},
                "memberCreator": {"fullName": "Contributor A", "username": "contrib_a"}
            },
            {
                "id": "act-2",
                "type": "commentCard",
                "date": "2026-06-03T09:00:00.000Z",
                "data": {"text": "Verified staging.", "card": {"id": "card-1"}},
                "memberCreator": {"fullName": "", "username": "reviewer_b"}
            },
            {
                "id": "act-3",
                "type": "commentCard",
                "date": "2026-06-03T10:00:00.000Z",
                "data": {"text": "Comment on closed card", "card": {"id": "card-closed"}},
                "memberCreator": {"fullName": "Contributor A"}
            },
            {
                "id": "act-4",
                "type": "updateCard",
                "date": "2026-06-03T11:00:00.000Z",
                "data": {"card": {"id": "card-1"}}
            }
        ]
    });
    trello_json.to_string().replace("TRAP_IMG", trap_img_url)
}

async fn assert_trello_board_properties(ts: &TestServer) -> Container {
    let boards: Vec<Container> = ts.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    assert_eq!(boards.len(), 1, "one board visible via API");
    let board = boards.into_iter().next().expect("board present");
    assert_eq!(board.kind, ContainerKind::Board, "container is Board");
    assert_eq!(board.title, "Product Launch", "board title preserved");
    assert_eq!(
        board.description, "Quarterly release checklist and tasks.",
        "board description preserved"
    );
    let prop_types: Vec<(&str, &str)> = board
        .card_properties
        .iter()
        .map(|prop| (prop.name.as_str(), prop.property_type.as_str()))
        .collect();
    assert_eq!(
        prop_types,
        vec![
            ("Status", "select"),
            ("Labels", "multiSelect"),
            ("Due", "date")
        ],
        "board card properties created for lists, labels, and due dates"
    );
    let status_opts = &board.card_properties[0].options;
    assert_eq!(
        status_opts.len(),
        2,
        "closed list excluded from status options"
    );
    board
}

async fn assert_trello_cards_and_comments(
    ts: &TestServer,
    board: &Container,
    importer_id: &str,
    trap_att_url: &str,
) {
    let blocks: Vec<Block> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .ok_json();
    let ship_card = blocks
        .iter()
        .find(|b| b.block_type == BlockType::Card && b.title == "Ship v1 release")
        .expect("Ship v1 release card stored");
    let due_id = board.card_properties[2].id.as_str();
    assert_eq!(
        ship_card.fields["properties"][due_id]["from"], "2026-06-01",
        "start date stored on card"
    );
    assert_eq!(
        ship_card.fields["properties"][due_id]["to"], "2026-06-15",
        "due date stored on card"
    );

    let card_body = ship_card.body.as_ref().expect("card has body").document();
    let mut task_items = Vec::new();
    find_nodes_by_type(card_body, "taskItem", &mut task_items);
    assert_eq!(task_items.len(), 2, "checklist items converted to taskItem");
    assert_eq!(
        task_items[0]["attrs"]["checked"], true,
        "first item complete"
    );
    assert_eq!(task_items[1]["attrs"]["checked"], false, "second item open");
    let mut tables = Vec::new();
    find_nodes_by_type(card_body, "table", &mut tables);
    assert_eq!(tables.len(), 1, "Markdown table inside card preserved");
    let body_text = serde_json::to_string(card_body).expect("serialize card body");
    assert!(
        body_text.contains("https://example.com/docs") && body_text.contains(trap_att_url),
        "external attachment URLs kept as links in body"
    );
    assert!(
        !body_text.contains("\"type\":\"file\"") && !body_text.contains("\"type\":\"image\""),
        "no fetched file or image blocks created from external URLs"
    );

    let comments: Vec<&Block> = blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Comment)
        .collect();
    assert_eq!(comments.len(), 2, "two comments stored on card");
    for comment in &comments {
        assert_eq!(comment.parent_id, ship_card.id, "comment parent is card");
        assert_eq!(
            comment.created_by, importer_id,
            "comment created_by is importer"
        );
        assert_eq!(
            comment.modified_by, importer_id,
            "comment modified_by is importer"
        );
    }
    let comment_texts: Vec<String> = comments
        .iter()
        .map(|c| {
            serde_json::to_string(c.body.as_ref().expect("comment body").document()).expect("json")
        })
        .collect();
    assert!(
        comment_texts
            .iter()
            .any(|t| t.contains("가져옴: Contributor A")),
        "fullName preserved as text attribution"
    );
    assert!(
        comment_texts
            .iter()
            .any(|t| t.contains("가져옴: reviewer_b")),
        "username fallback preserved as text attribution"
    );
}

#[tokio::test]
async fn trello_json_round_trip_imports_board_properties_checklists_comments_and_blocks_ssrf() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let (trap_addr, trap_hits, trap_task) = spawn_ssrf_trap().await;
    let trap_img_url = format!("http://{trap_addr}/ssrf-image.png");
    let trap_att_url = format!("http://{trap_addr}/ssrf-secret.pdf");

    let tmp = TestDir::new();
    let json_path = tmp.path().join("trello.json");
    std::fs::write(
        &json_path,
        build_trello_fixture(&trap_img_url, &trap_att_url),
    )
    .expect("write trello fixture");

    let summary = ts
        .app
        .import_external_archive(
            &json_path,
            ImportFormat::Trello,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("Trello import succeeds");

    assert_eq!(summary.format, ImportFormat::Trello, "summary format");
    assert_eq!(summary.imported.boards, 1, "one board imported");
    assert_eq!(summary.imported.wikis, 0, "zero wikis imported");
    assert_eq!(summary.imported.cards, 2, "two open cards imported");
    assert_eq!(summary.imported.comments, 2, "two comments imported");
    assert_eq!(summary.imported.files, 0, "no remote files fetched");
    assert_eq!(summary.container_ids.len(), 1, "one container ID returned");
    assert_eq!(
        trap_hits.load(Ordering::SeqCst),
        0,
        "SSRF trap must receive zero connections"
    );
    assert!(
        summary
            .skipped
            .iter()
            .filter(|w| w.code == ImportWarningCode::ClosedItemSkipped)
            .count()
            >= 3,
        "closed list, closed card, and card in closed list reported as skipped"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::ExternalAttachmentLinked),
        "external attachments recorded as warnings"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::ExternalImageLinked),
        "external image recorded as warning"
    );

    let board = assert_trello_board_properties(&ts).await;
    assert_trello_cards_and_comments(&ts, &board, &importer_id, &trap_att_url).await;
    trap_task.abort();
    ts.teardown().await;
}

struct NotionFixtureAssets {
    outer_zip: Vec<u8>,
    png_bytes: &'static [u8],
    pdf_bytes: &'static [u8],
    txt_bytes: &'static [u8],
}

fn build_notion_fixture(trap_img_url: &str) -> NotionFixtureAssets {
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let handbook_md = format!(
        "# Handbook\n\nWelcome to the guide.\n\n\
         | Service | Port |\n| --- | --- |\n| API | 8080 |\n| Worker | 9090 |\n\n\
         See [Architecture](Handbook%200123456789abcdef0123456789abcdef/Architecture%20fedcba9876543210fedcba9876543210.md).\n\n\
         ![Diagram](Handbook%200123456789abcdef0123456789abcdef/diagram.png)\n\n\
         [Spec PDF](Handbook%200123456789abcdef0123456789abcdef/spec.pdf)\n\n\
         ![Remote]({trap_img_url})\n\n\
         ![Missing](missing-local.png)\n"
    );
    let arch_md =
        "# Architecture\n\n```rust\nfn main() {}\n```\n\n- [x] Designed\n- [ ] Reviewed\n";
    let deep_md = "# Deep Guide\n\nNested under synthetic folder page.\n";
    let csv_short = "Name,Status\nLaunch Checklist,In Progress\n";
    let csv_all = "\u{FEFF}Name,Status,Tags,Timeline,Done,Points,Link,Notes\n\
Launch Checklist,In Progress,\"Web, API\",2026-07-01 → 2026-07-10,Yes,13,https://example.com/launch,Track all rollout steps carefully.\n\
Security Audit,Done,Security,\"July 20, 2026\",No,$500,https://example.com/audit,Review threat model thoroughly.\n\
Docs Polish,In Progress,Docs,2026-07-25,Yes,5,https://example.com/docs,Update onboarding guide text.\n";
    let launch_row_md =
        "# Launch Checklist\n\nVerify rollout.\n\n[Checklist File](checklist.txt)\n";

    let png_bytes: &'static [u8] = b"\x89PNG\r\n\x1a\nfake-png-payload";
    let pdf_bytes: &'static [u8] = b"%PDF-1.7\nfake-pdf-payload";
    let txt_bytes: &'static [u8] = b"step 1: verify\nstep 2: release\n";

    let inner_zip = build_zip(&[
        (
            "Workspace Export/Handbook 0123456789abcdef0123456789abcdef.md",
            handbook_md.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Handbook 0123456789abcdef0123456789abcdef/Architecture fedcba9876543210fedcba9876543210.md",
            arch_md.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Handbook 0123456789abcdef0123456789abcdef/diagram.png",
            png_bytes,
            deflated,
        ),
        (
            "Workspace Export/Handbook 0123456789abcdef0123456789abcdef/spec.pdf",
            pdf_bytes,
            deflated,
        ),
        (
            "Workspace Export/Guides/Deep Guide aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.md",
            deep_md.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Roadmap 11111111111111111111111111111111.csv",
            csv_short.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Roadmap 11111111111111111111111111111111_all.csv",
            csv_all.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Roadmap 11111111111111111111111111111111/Launch Checklist 22222222222222222222222222222222.md",
            launch_row_md.as_bytes(),
            deflated,
        ),
        (
            "Workspace Export/Roadmap 11111111111111111111111111111111/checklist.txt",
            txt_bytes,
            deflated,
        ),
    ]);
    let outer_zip = build_zip(&[("Export-nested-part1.zip", &inner_zip, deflated)]);
    NotionFixtureAssets {
        outer_zip,
        png_bytes,
        pdf_bytes,
        txt_bytes,
    }
}

async fn assert_notion_wiki_tree(ts: &TestServer) -> (Container, String, String) {
    let wikis: Vec<Container> = ts.client.get(&format!("{TEST_WS}/wikis")).await.ok_json();
    assert_eq!(wikis.len(), 1, "one wiki stored");
    let wiki = wikis.into_iter().next().expect("wiki present");
    let tree: Vec<PageNode> = ts
        .client
        .get(&format!("{TEST_WS}/wikis/{}/pages", wiki.id))
        .await
        .ok_json();
    assert_eq!(tree.len(), 4, "wiki tree has 4 pages");
    let handbook_node = tree
        .iter()
        .find(|p| p.title == "Handbook")
        .expect("Handbook page");
    let arch_node = tree
        .iter()
        .find(|p| p.title == "Architecture")
        .expect("Architecture page");
    let guides_node = tree
        .iter()
        .find(|p| p.title == "Guides")
        .expect("synthetic Guides page");
    let deep_node = tree
        .iter()
        .find(|p| p.title == "Deep Guide")
        .expect("Deep Guide page");
    assert_eq!(
        arch_node.parent_id, handbook_node.id,
        "Architecture nested under Handbook"
    );
    assert_eq!(
        deep_node.parent_id, guides_node.id,
        "Deep Guide nested under Guides"
    );
    let handbook_id = handbook_node.id.clone();
    let arch_id = arch_node.id.clone();
    (wiki, handbook_id, arch_id)
}

async fn assert_notion_wiki_body_and_files(
    ts: &TestServer,
    wiki: &Container,
    handbook_id: &str,
    arch_id: &str,
    png_bytes: &[u8],
    pdf_bytes: &[u8],
) {
    let wiki_blocks = ts
        .app
        .store()
        .get_blocks(&fvoci_store::BlockFilter::container(&wiki.id))
        .await
        .expect("read wiki blocks");
    for page in wiki_blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Page)
    {
        assert!(
            page.fields
                .get(PAGE_ORDER_FIELD)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            "every wiki page carries a non-empty order key"
        );
    }
    let handbook_res = ts
        .client
        .get(&format!("{TEST_WS}/wikis/{}/pages/{handbook_id}", wiki.id))
        .await;
    handbook_res.check_ok();
    let hb_body = &handbook_res.value()["body"];

    let mut tables = Vec::new();
    find_nodes_by_type(hb_body, "table", &mut tables);
    assert_eq!(tables.len(), 1, "Handbook contains B1 table node");
    let mut headers = Vec::new();
    find_nodes_by_type(hb_body, "tableHeader", &mut headers);
    assert_eq!(headers.len(), 2, "table has 2 header cells");
    let mut cells = Vec::new();
    find_nodes_by_type(hb_body, "tableCell", &mut cells);
    assert_eq!(cells.len(), 4, "table has 4 body cells");

    let mut refs = Vec::new();
    find_nodes_by_type(hb_body, "reference", &mut refs);
    assert_eq!(refs.len(), 1, "internal link converted to reference node");
    assert_eq!(
        refs[0]["attrs"]["id"], arch_id,
        "reference node points to remapped Architecture page ID"
    );

    let mut imgs = Vec::new();
    find_nodes_by_type(hb_body, "image", &mut imgs);
    assert_eq!(imgs.len(), 1, "only local image emitted as image block");
    let img_file_id = imgs[0]["attrs"]["fileId"].as_str().expect("image fileId");
    let downloaded_png = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{img_file_id}",
            wiki.id
        ))
        .await;
    downloaded_png.check_ok();
    assert_eq!(downloaded_png.body, png_bytes, "local PNG round-tripped");

    let mut files = Vec::new();
    find_nodes_by_type(hb_body, "file", &mut files);
    assert_eq!(files.len(), 1, "local PDF emitted as file block");
    let pdf_file_id = files[0]["attrs"]["fileId"].as_str().expect("pdf fileId");
    let downloaded_pdf = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{pdf_file_id}",
            wiki.id
        ))
        .await;
    downloaded_pdf.check_ok();
    assert_eq!(downloaded_pdf.body, pdf_bytes, "local PDF round-tripped");
}

async fn assert_notion_board(ts: &TestServer, txt_bytes: &[u8]) {
    let boards: Vec<Container> = ts.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    assert_eq!(boards.len(), 1, "one board imported from CSV database");
    let board = &boards[0];
    assert_eq!(
        board.title, "Roadmap",
        "Notion ID suffix stripped from board title"
    );
    let inferred: HashSet<&str> = board
        .card_properties
        .iter()
        .map(|p| p.property_type.as_str())
        .collect();
    for expected in [
        "select",
        "multiSelect",
        "date",
        "checkbox",
        "number",
        "url",
        "text",
    ] {
        assert!(
            inferred.contains(expected),
            "CSV board inferred property type {expected}"
        );
    }

    let board_blocks: Vec<Block> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .ok_json();
    let launch_card = board_blocks
        .iter()
        .find(|b| b.block_type == BlockType::Card && b.title == "Launch Checklist")
        .expect("Launch Checklist card imported");
    let card_body = launch_card
        .body
        .as_ref()
        .expect("companion row Markdown attached as card body")
        .document();
    let mut card_files = Vec::new();
    find_nodes_by_type(card_body, "file", &mut card_files);
    assert_eq!(card_files.len(), 1, "companion card local file attached");
    let txt_file_id = card_files[0]["attrs"]["fileId"]
        .as_str()
        .expect("txt fileId");
    let downloaded_txt = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{txt_file_id}",
            board.id
        ))
        .await;
    downloaded_txt.check_ok();
    assert_eq!(
        downloaded_txt.body, txt_bytes,
        "companion card file bytes round-tripped"
    );
}

#[tokio::test]
async fn notion_zip_round_trip_imports_wiki_tree_tables_assets_and_csv_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let (trap_addr, trap_hits, trap_task) = spawn_ssrf_trap().await;
    let trap_img_url = format!("http://{trap_addr}/notion-pixel.png");
    let fixture = build_notion_fixture(&trap_img_url);

    let tmp = TestDir::new();
    let zip_path = tmp.path().join("notion-export.zip");
    std::fs::write(&zip_path, &fixture.outer_zip).expect("write notion zip");

    let summary = ts
        .app
        .import_external_archive(
            &zip_path,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("Notion import succeeds");

    assert_eq!(
        trap_hits.load(Ordering::SeqCst),
        0,
        "SSRF trap must receive zero connections"
    );
    assert_eq!(summary.format, ImportFormat::Notion, "summary format");
    assert_eq!(summary.imported.wikis, 1, "one wiki created");
    assert_eq!(summary.imported.boards, 1, "one board created");
    assert_eq!(
        summary.imported.pages, 4,
        "Handbook + Architecture + synthetic Guides + Deep Guide = 4 pages"
    );
    assert_eq!(
        summary.imported.cards, 3,
        "three CSV rows imported as cards"
    );
    assert_eq!(
        summary.imported.files, 3,
        "diagram.png, spec.pdf, checklist.txt stored"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::ExternalImageLinked),
        "external image warning emitted"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::MissingLocalAsset),
        "missing local asset warning emitted"
    );
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::UnsupportedEntrySkipped),
        "duplicate non-_all CSV skipped with warning"
    );

    let (wiki, handbook_id, arch_id) = assert_notion_wiki_tree(&ts).await;
    assert_notion_wiki_body_and_files(
        &ts,
        &wiki,
        &handbook_id,
        &arch_id,
        fixture.png_bytes,
        fixture.pdf_bytes,
    )
    .await;
    assert_notion_board(&ts, fixture.txt_bytes).await;

    trap_task.abort();
    ts.teardown().await;
}

#[tokio::test]
async fn adversarial_traversal_and_symlinks_leave_zero_state() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let tmp = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let traversal_cases = [
        ("dotdot", "../etc/passwd", b"root:x:0:0".as_slice(), stored),
        ("abs", "/etc/passwd", b"root:x:0:0".as_slice(), stored),
        (
            "nested_dotdot",
            "a/../../escape.md",
            b"# nope".as_slice(),
            stored,
        ),
        ("backslash", "a\\b.md", b"# nope".as_slice(), stored),
        ("drive", "C:/evil.md", b"# nope".as_slice(), stored),
    ];
    for (name, entry_name, content, opts) in traversal_cases {
        let path = tmp.path().join(format!("{name}.zip"));
        std::fs::write(&path, build_zip(&[(entry_name, content, opts)]))
            .expect("write traversal zip");
        let err = ts
            .app
            .import_external_archive(&path, ImportFormat::Notion, TEST_WORKSPACE_ID, &importer_id)
            .await
            .expect_err("traversal zip must be rejected");
        assert_eq!(err.kind(), ErrorKind::BadRequest, "{name} rejected as 400");
    }

    let symlink_path = tmp.path().join("symlink.zip");
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link.md", "/etc/passwd", stored)
            .expect("add symlink");
        std::fs::write(&symlink_path, writer.finish().expect("finish").into_inner())
            .expect("write symlink zip");
    }
    let err = ts
        .app
        .import_external_archive(
            &symlink_path,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("symlink zip must be rejected");
    assert_eq!(err.kind(), ErrorKind::BadRequest, "symlink rejected as 400");

    assert_nothing_stored(&ts, "after traversal and symlink attempts").await;
    ts.teardown().await;
}

#[tokio::test]
async fn adversarial_zip_bombs_and_bounds_leave_zero_state() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let tmp = TestDir::new();
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let bomb_path = tmp.path().join("bomb.zip");
    let zeros = vec![0_u8; 4 * 1024 * 1024];
    std::fs::write(&bomb_path, build_zip(&[("bomb.md", &zeros, deflated)])).expect("write bomb");
    let err = ts
        .app
        .import_external_archive(
            &bomb_path,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("compression bomb must be rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "zip bomb rejected as 413"
    );

    let total_path = tmp.path().join("total.zip");
    let payload = vec![b'x'; 2_000];
    std::fs::write(
        &total_path,
        build_zip(&[("a.md", &payload, stored), ("b.md", &payload, stored)]),
    )
    .expect("write total zip");
    let err = ts
        .app
        .import_external_archive_with_limits(
            &total_path,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
            BundleLimits {
                max_total_unpacked_bytes: 3_000,
                ..BundleLimits::default()
            },
        )
        .await
        .expect_err("total unpacked limit enforced");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "total unpacked is 413"
    );

    let err = ts
        .app
        .import_external_archive_with_limits(
            &total_path,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
            BundleLimits {
                max_entries: 1,
                ..BundleLimits::default()
            },
        )
        .await
        .expect_err("entry count limit enforced");
    assert_eq!(err.kind(), ErrorKind::PayloadTooLarge, "entry count is 413");

    assert_nothing_stored(&ts, "after zip bomb and bound violations").await;
    ts.teardown().await;
}

#[tokio::test]
async fn adversarial_payload_limits_are_rejected_with_zero_stored_state() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let tmp = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let deep_md = format!("{}boom", "> ".repeat(60));
    let deep_md_zip = tmp.path().join("deep-md.zip");
    std::fs::write(
        &deep_md_zip,
        build_zip(&[("page.md", deep_md.as_bytes(), stored)]),
    )
    .expect("write deep md zip");
    let err = ts
        .app
        .import_external_archive(
            &deep_md_zip,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("deep markdown rejected");
    assert_eq!(err.kind(), ErrorKind::BadRequest, "deep markdown is 400");

    let huge_trello = tmp.path().join("huge-trello.json");
    std::fs::write(
        &huge_trello,
        format!("{{\"name\":\"{}\"}}", "a".repeat(2_000)),
    )
    .expect("write huge trello");
    let err = ts
        .app
        .import_external_archive_with_limits(
            &huge_trello,
            ImportFormat::Trello,
            TEST_WORKSPACE_ID,
            &importer_id,
            BundleLimits {
                max_trello_json_bytes: 512,
                ..BundleLimits::default()
            },
        )
        .await
        .expect_err("oversized Trello JSON rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "oversized Trello JSON is 413"
    );

    let deep_trello = tmp.path().join("deep-trello.json");
    let nested_json = format!(
        "{{\"name\":\"Deep\",\"extra\":{}{}}}",
        "[".repeat(80),
        "]".repeat(80)
    );
    std::fs::write(&deep_trello, nested_json).expect("write deep trello");
    let err = ts
        .app
        .import_external_archive(
            &deep_trello,
            ImportFormat::Trello,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("deeply nested Trello JSON rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::BadRequest,
        "deeply nested JSON is 400"
    );

    let big_csv_zip = tmp.path().join("big-csv.zip");
    std::fs::write(
        &big_csv_zip,
        build_zip(&[("db.csv", b"Name,Col\nA,1\nB,2\nC,3\n", stored)]),
    )
    .expect("write big csv zip");
    let err = ts
        .app
        .import_external_archive_with_limits(
            &big_csv_zip,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
            BundleLimits {
                max_csv_rows: 2,
                ..BundleLimits::default()
            },
        )
        .await
        .expect_err("oversized CSV rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "oversized CSV is 413"
    );

    assert_nothing_stored(&ts, "after adversarial JSON/Markdown/CSV payloads").await;
    ts.teardown().await;
}

#[tokio::test]
async fn mid_import_failure_permissions_and_upload_bridge_preserve_atomicity() {
    let ts = Setup::native()
        .configure(|config| {
            config.wiki.wiki_page_depth = 2;
        })
        .start()
        .await
        .init_basic()
        .await;
    let importer_id = ts.client.me().await.id;
    let tmp = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // 1. Atomicity: a Notion bundle with a valid Board + local file AND a Wiki violating
    //    `wiki_page_depth = 2` (3 levels deep) must leave 0 boards, 0 wikis, and 0 files!
    let atomic_zip = tmp.path().join("atomic.zip");
    std::fs::write(
        &atomic_zip,
        build_zip(&[
            ("Tasks.csv", b"Name,Status\nItem 1,Todo\n", stored),
            ("Tasks/Item 1.md", b"# Item 1\n\n[Doc](doc.txt)\n", stored),
            ("Tasks/doc.txt", b"should never remain in storage", stored),
            ("L1/L2/L3.md", b"# Too deep for wiki_page_depth=2", stored),
        ]),
    )
    .expect("write atomic zip");
    let err = ts
        .app
        .import_external_archive(
            &atomic_zip,
            ImportFormat::Notion,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("wiki depth violation rejects entire bundle");
    assert_eq!(
        err.kind(),
        ErrorKind::BadRequest,
        "wiki depth violation is 400"
    );
    assert_nothing_stored(&ts, "after mid-import wiki depth failure").await;

    // 2. Permission boundaries: non-member, guest, and unknown workspace all get 404.
    let valid_trello = tmp.path().join("valid-trello.json");
    let trello_bytes = br#"{"name":"Perm Check","lists":[{"id":"l1","name":"Todo"}],"cards":[{"id":"c1","name":"One","idList":"l1"}]}"#;
    std::fs::write(&valid_trello, trello_bytes).expect("write valid trello");

    let outsider_id = ts.stranger().await;
    let err = ts
        .app
        .import_external_archive(
            &valid_trello,
            ImportFormat::Trello,
            TEST_WORKSPACE_ID,
            &outsider_id,
        )
        .await
        .expect_err("non-member rejected");
    assert_eq!(err.kind(), ErrorKind::NotFound, "non-member gets 404");

    ts.app
        .store()
        .add_workspace_member(TEST_WORKSPACE_ID, &outsider_id, WorkspaceRole::Guest)
        .await
        .expect("add guest");
    let err = ts
        .app
        .import_external_archive(
            &valid_trello,
            ImportFormat::Trello,
            TEST_WORKSPACE_ID,
            &outsider_id,
        )
        .await
        .expect_err("guest rejected");
    assert_eq!(err.kind(), ErrorKind::NotFound, "guest gets 404");

    let err = ts
        .app
        .import_external_archive(
            &valid_trello,
            ImportFormat::Trello,
            UNKNOWN_ID,
            &importer_id,
        )
        .await
        .expect_err("unknown workspace rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::NotFound,
        "unknown workspace gets 404"
    );
    assert_nothing_stored(&ts, "after permission boundary rejections").await;

    ts.teardown().await;
}

#[tokio::test]
async fn import_upload_with_format_imports_trello_and_cleans_up_temp_files() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let trello_bytes = br#"{"name":"Upload Bridge","lists":[{"id":"l1","name":"Todo"}],"cards":[{"id":"c1","name":"One","idList":"l1"}]}"#;

    let upload = ts
        .app
        .create_import_upload(
            &importer_id,
            TEST_WORKSPACE_ID,
            FileToUpload {
                name: "board.json".to_owned(),
                size: i64::try_from(trello_bytes.len()).expect("small size"),
                content_type: "application/json".to_owned(),
            },
        )
        .await
        .expect("open import upload");
    put_part(&ts.client, TEST_WORKSPACE_ID, &upload.id, 1, trello_bytes)
        .await
        .check_no_content();
    let summary = ts
        .app
        .import_upload_with_format(&upload, ImportFormat::Trello)
        .await
        .expect("import_upload_with_format succeeds");
    assert_eq!(summary.imported.boards, 1, "upload bridge imported 1 board");
    assert_eq!(summary.imported.cards, 1, "upload bridge imported 1 card");

    let uploads_dir = ts.app.config().tmp_dir().join("uploads");
    let leftover = std::fs::read_dir(&uploads_dir).map_or(0, Iterator::count);
    assert_eq!(leftover, 0, "upload temporary directory cleaned up");

    ts.teardown().await;
}
