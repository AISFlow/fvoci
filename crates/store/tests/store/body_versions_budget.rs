//! Per-card byte budget for `body_versions` (`body_bytes`), and PostgreSQL TOAST
//! disk-usage measurement for 4 MiB × 50 versions.

use fvoci_model::{BlockPatch, Body, BodyChange, BodyVersionCause};
use fvoci_store::{
    ByHand, DEFAULT_BODY_VERSIONS_KEPT, DEFAULT_BODY_VERSIONS_MAX_BYTES, DbType, Store,
};
use serde_json::{Value, json};
use sqlx::Connection as _;

use crate::body_versions::{board_with_files, body_showing, card_with, keep, type_body};
use crate::common;

/// Writes the card's body whole, by the user.
async fn write_body(store: &Store, card_id: &str, body: Option<Body>, user_id: &str) {
    let patch = BlockPatch {
        body: body.map_or(BodyChange::Clear, BodyChange::Set),
        ..BlockPatch::default()
    };
    store
        .patch_block(card_id, &patch, user_id)
        .await
        .expect("the body is written");
}

/// Serialized JSON UTF-8 byte length of a body, or `0` for `None`.
fn body_json_bytes(body: Option<&Body>) -> u64 {
    body.map_or(0, |doc| {
        u64::try_from(serde_json::to_string(doc.document()).expect("json").len()).expect("fits u64")
    })
}

/// The stored `(version, body_bytes)` rows for a card, newest first.
async fn stored_versions_and_bytes(store: &Store, card_id: &str) -> Vec<(i64, i64)> {
    let rows = store
        .texts_unchecked(&format!(
            "SELECT CAST(version AS TEXT) || ':' || CAST(body_bytes AS TEXT) \
             FROM body_versions WHERE block_id = '{card_id}' ORDER BY version DESC"
        ))
        .await
        .expect("versions read");
    rows.into_iter()
        .map(|entry| {
            let (version, bytes) = entry.split_once(':').expect("version:bytes");
            (
                version.parse::<i64>().expect("version"),
                bytes.parse::<i64>().expect("bytes"),
            )
        })
        .collect()
}

/// Files unreferenced as of an hour from now.
async fn collect_files(store: &Store) -> Vec<String> {
    let later = crate::blocks::now_millis() + 3_600_000;
    let mut names: Vec<String> = store
        .collect_unreferenced_files(later)
        .await
        .expect("collected")
        .into_iter()
        .map(|file| file.stored_name)
        .collect();
    names.sort();
    names
}

/// Byte budget prunes oldest versions even while below the version count limit, removes
/// their `body_version_editors` rows, and releases files only those versions showed while
/// preserving files referenced by kept versions.
#[tokio::test]
async fn byte_budget_prunes_oldest_versions_editors_and_unreferenced_files() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "budget.author").await;
    let editor = common::user(store, "budget.editor").await;
    let board_id =
        board_with_files(store, &author, &["7first.png", "7second.png", "7third.png"]).await;

    let pad = "a".repeat(200);
    let b1 = body_showing(&format!("v1-{pad}"), &["7first.png"]);
    let b2 = body_showing(&format!("v2-{pad}"), &["7second.png"]);
    let b3 = body_showing(&format!("v3-{pad}"), &["7third.png"]);
    let size1 = body_json_bytes(b1.as_ref());
    let size2 = body_json_bytes(b2.as_ref());
    let size3 = body_json_bytes(b3.as_ref());

    // Keep count high (default 50) so only byte budget triggers: fits exactly 2 versions.
    store.set_body_versions_kept(DEFAULT_BODY_VERSIONS_KEPT);
    store.set_body_versions_max_bytes(size2 + size3);

    // Give v1 a recorded editor row so pruning v1 genuinely removes an editor row.
    let card_id = card_with(store, &board_id, None, &author).await;
    type_body(store, &card_id, b1.clone(), &author).await;
    assert_eq!(keep(store, &card_id).await, Some(1), "v1 kept");
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("counted"),
        1,
        "v1 has an editor row before pruning"
    );

    type_body(store, &card_id, b2.clone(), &editor).await;
    assert_eq!(keep(store, &card_id).await, Some(2), "v2 kept");
    assert_eq!(
        stored_versions_and_bytes(store, &card_id).await,
        [
            (2, i64::try_from(size2).expect("i64")),
            (1, i64::try_from(size1).expect("i64")),
        ],
        "both fit within budget"
    );
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("counted"),
        2,
        "v1 and v2 both have editor rows before v3 triggers pruning"
    );

    // Adding v3 pushes total over budget; v1 and its editor row + file ref are removed.
    type_body(store, &card_id, b3.clone(), &author).await;
    assert_eq!(keep(store, &card_id).await, Some(3), "v3 kept");
    assert_eq!(
        stored_versions_and_bytes(store, &card_id).await,
        [
            (3, i64::try_from(size3).expect("i64")),
            (2, i64::try_from(size2).expect("i64")),
        ],
        "v1 pruned by byte budget"
    );
    assert_eq!(
        collect_files(store).await,
        ["7first.png"],
        "v1's file is now collectable; v2 and v3 files stay protected"
    );
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("counted"),
        2,
        "v1's editor row was deleted as v3's row was added"
    );

    let (v2_meta, v2_body) = store
        .body_version(&card_id, 2)
        .await
        .expect("read")
        .expect("v2 exists");
    assert_eq!(
        v2_meta.editors,
        std::slice::from_ref(&editor),
        "v2 retains its editor"
    );
    assert_eq!(
        serde_json::to_string(v2_body.expect("body").document()).expect("json"),
        serde_json::to_string(b2.expect("b2").document()).expect("json"),
        "v2 body round-trips byte-for-byte"
    );
    store.set_body_versions_max_bytes(DEFAULT_BODY_VERSIONS_MAX_BYTES);
    test.teardown().await;
}

/// Whole-body writes (`patch_block`) also enforce the byte budget, clean up pruned
/// editor rows, and release files shown only by pruned versions.
#[tokio::test]
async fn byte_budget_prunes_whole_body_writes_and_releases_files() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "whole.author").await;
    let editor = common::user(store, "whole.editor").await;
    let board_id = board_with_files(store, &author, &["7old.png", "7keep.png"]).await;

    let pad = "w".repeat(200);
    let b1 = body_showing(&format!("x1-{pad}"), &["7old.png"]);
    let b2 = body_showing(&format!("x2-{pad}"), &["7keep.png"]);
    let b3 = body_showing(&format!("x3-{pad}"), &[]);
    store.set_body_versions_kept(DEFAULT_BODY_VERSIONS_KEPT);
    store.set_body_versions_max_bytes(body_json_bytes(b1.as_ref()) + body_json_bytes(b2.as_ref()));

    let card_id = card_with(store, &board_id, None, &author).await;
    write_body(store, &card_id, b1, &author).await;
    write_body(store, &card_id, b2, &editor).await;
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("counted"),
        2,
        "v1 and v2 both have editor rows"
    );
    write_body(store, &card_id, b3, &author).await;
    let kept_versions = store.body_versions(&card_id).await.expect("versions");
    assert_eq!(
        kept_versions.iter().map(|v| v.version).collect::<Vec<_>>(),
        [3, 2],
        "write_body enforces byte budget"
    );
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("counted"),
        2,
        "v1's editor row removed when pruned by write_body"
    );
    assert_eq!(
        collect_files(store).await,
        ["7old.png"],
        "v1's file released; v2's 7keep.png preserved"
    );
    test.teardown().await;
}

/// A single version larger than the byte budget is never deleted immediately: the newest
/// version always stays, while all older versions are pruned.
#[tokio::test]
async fn single_version_exceeding_budget_always_keeps_newest() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "single.author").await;
    let board_id = board_with_files(store, &author, &["7gone.png", "7newest.png"]).await;

    let small = body_showing("small", &["7gone.png"]);
    let huge = body_showing(&"z".repeat(1024), &["7newest.png"]);
    let huge_bytes = body_json_bytes(huge.as_ref());

    // Setting 0 clamps to 1 byte, far smaller than any valid body JSON.
    store.set_body_versions_max_bytes(0);
    let card_id = card_with(store, &board_id, huge.clone(), &author).await;
    assert_eq!(keep(store, &card_id).await, Some(1));
    assert_eq!(
        stored_versions_and_bytes(store, &card_id).await,
        [(1, i64::try_from(huge_bytes).expect("i64"))],
        "first oversized version is kept"
    );

    // Budget fits small + empty, then a huge version arrives: older versions go, newest stays.
    let second_card = card_with(store, &board_id, small.clone(), &author).await;
    store.set_body_versions_max_bytes(body_json_bytes(small.as_ref()) + 64);
    assert_eq!(keep(store, &second_card).await, Some(1));
    type_body(store, &second_card, None, &author).await;
    assert_eq!(keep(store, &second_card).await, Some(2));
    assert_eq!(
        stored_versions_and_bytes(store, &second_card).await.len(),
        2,
        "zero-byte cleared version stays with small version"
    );

    type_body(store, &second_card, huge.clone(), &author).await;
    assert_eq!(keep(store, &second_card).await, Some(3));
    assert_eq!(
        stored_versions_and_bytes(store, &second_card).await,
        [(3, i64::try_from(huge_bytes).expect("i64"))],
        "all older versions including zero-byte version pruned when newest exceeds budget"
    );
    assert_eq!(
        collect_files(store).await,
        ["7gone.png"],
        "newest oversized version still protects its file; older file is collected"
    );
    test.teardown().await;
}

/// `(version, cause)` pairs for a card's kept versions, newest first.
async fn version_causes(store: &Store, card_id: &str) -> Vec<(i64, BodyVersionCause)> {
    store
        .body_versions(card_id)
        .await
        .expect("listed")
        .into_iter()
        .map(|v| (v.version, v.cause))
        .collect()
}

/// Keeps a version by hand with no recency floor.
async fn keep_by_hand(store: &Store, card_id: &str) -> Option<ByHand> {
    store
        .keep_body_version_by_hand(card_id, 0)
        .await
        .expect("by hand")
}

/// Count limit and byte budget compose cleanly (whichever is stricter wins), and
/// `keep` / `list` / `read` / `restore` / `keep_by_hand` preserve behavior and exact bytes.
#[tokio::test]
async fn count_and_byte_limits_compose_across_all_version_operations() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "combo.author").await;
    let editor = common::user(store, "combo.editor").await;
    let board_id = board_with_files(store, &author, &["7a.png", "7b.png", "7c.png"]).await;

    let big = body_showing(&"m".repeat(500), &["7c.png"]);
    let big_bytes = body_json_bytes(big.as_ref());

    // First: count limit (3) is stricter than generous byte budget.
    store.set_body_versions_kept(3);
    store.set_body_versions_max_bytes(big_bytes + 300);
    let card_id = card_with(
        store,
        &board_id,
        body_showing("v1-short", &["7a.png"]),
        &author,
    )
    .await;
    assert_eq!(keep(store, &card_id).await, Some(1), "v1 kept");
    type_body(
        store,
        &card_id,
        body_showing("v2-short", &["7b.png"]),
        &editor,
    )
    .await;
    assert_eq!(
        keep_by_hand(store, &card_id).await,
        Some(ByHand::Kept(2)),
        "v2 kept by hand"
    );
    type_body(store, &card_id, body_showing("v3-short", &[]), &editor).await;
    assert_eq!(keep(store, &card_id).await, Some(3), "v3 kept");
    type_body(store, &card_id, body_showing("v4-short", &[]), &author).await;
    assert_eq!(keep(store, &card_id).await, Some(4), "v4 kept");
    assert_eq!(
        version_causes(store, &card_id).await,
        [
            (4, BodyVersionCause::Auto),
            (3, BodyVersionCause::Auto),
            (2, BodyVersionCause::Manual),
        ],
        "count limit pruned v1"
    );

    // Next: restore a large body, then lower budget so byte budget beats count limit (3).
    let patch = BlockPatch {
        body: BodyChange::Set(big.clone().expect("body")),
        ..BlockPatch::default()
    };
    store
        .restore_body(&card_id, &patch, &author)
        .await
        .expect("restored");
    assert_eq!(
        version_causes(store, &card_id).await,
        [
            (5, BodyVersionCause::Restored),
            (4, BodyVersionCause::Auto),
            (3, BodyVersionCause::Auto),
        ],
        "v2 pruned; v5 recorded as restored"
    );
    let small_after = body_showing("v6-hand", &[]);
    store.set_body_versions_max_bytes(big_bytes + body_json_bytes(small_after.as_ref()));
    type_body(store, &card_id, small_after, &editor).await;
    assert_eq!(
        keep_by_hand(store, &card_id).await,
        Some(ByHand::Kept(6)),
        "v6 kept by hand"
    );
    assert_eq!(
        version_causes(store, &card_id).await,
        [
            (6, BodyVersionCause::Manual),
            (5, BodyVersionCause::Restored)
        ],
        "byte budget pruned v4 and v3 even though count limit is 3"
    );
    assert_eq!(
        store
            .count_rows("body_version_editors")
            .await
            .expect("editors"),
        2,
        "editors table has no orphan rows"
    );
    assert_eq!(
        collect_files(store).await,
        ["7a.png", "7b.png"],
        "7c.png stays protected by kept v5 while 7a.png and 7b.png are collectable"
    );
    let (v5_meta, v5_body) = store
        .body_version(&card_id, 5)
        .await
        .expect("read")
        .expect("v5");
    assert_eq!(v5_meta.cause, BodyVersionCause::Restored, "v5 cause intact");
    assert_eq!(
        serde_json::to_string(v5_body.expect("body").document()).expect("json"),
        serde_json::to_string(big.expect("big").document()).expect("json"),
        "v5 body round-trips byte-for-byte"
    );
    test.teardown().await;
}

/// Deterministic `SplitMix64` generator for reproducible benchmark documents.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn token(&mut self, len: usize) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(len);
        let mut bits = 0_u64;
        let mut left = 0_u32;
        for _ in 0..len {
            if left < 6 {
                bits = self.next_u64();
                left = 64;
            }
            out.push(char::from(
                ALPHABET[usize::try_from(bits & 63).expect("index")],
            ));
            bits >>= 6;
            left -= 6;
        }
        out
    }
}

const REALISTIC_SENTENCES: [&str; 16] = [
    "Architecture review notes covering storage layout, transaction boundaries, and incremental indexing behavior across large workspaces.",
    "설계 문서 초안: 카드 본문 버전 이력은 협업 세션 종료 시점과 주기적 스냅샷을 함께 보존하며 첨부 파일 참조 무결성을 유지한다.",
    "Query planner observations show primary-key range scans avoid touching external TOAST pages until full version payloads are requested.",
    "운영 점검 항목: 백업 복구 훈련 시 대용량 문서의 직렬화 크기와 압축 후 디스크 점유량 차이를 주기적으로 확인해야 한다.",
    "Client synchronization replays compact updates and projects validated documents without blocking concurrent readers on adjacent boards.",
    "편집 이력 정리 정책은 개수 상한과 카드당 바이트 예산을 함께 적용하되 가장 최신 판 하나는 어떤 경우에도 즉시 삭제하지 않는다.",
    "Checklist verification confirmed consistent behavior across SQLite local deployments and multi-tenant PostgreSQL clusters under RLS.",
    "성능 측정 결과 요약: 일반 기술 문서는 중복 구문과 JSON 키 반복 때문에 높은 압축률을 보이지만 고엔트로피 입력은 별도로 검증한다.",
    "Release checklists record schema migration compatibility, rollback caveats, and disk headroom thresholds before rolling upgrades begin.",
    "장애 대응 절차: 데이터베이스 저장 공간 경고가 발생하면 카드별 버전 바이트 합계 상위 항목을 먼저 확인하고 정리 정책을 점검한다.",
    "Background workers fold logged document updates on shutdown and recover unfinished projections cleanly during the next server startup.",
    "문서 편집기 호환성 점검: 제목, 문단, 목록, 할 일 목록, 인용구, 코드 블록, 인라인 링크와 강조 표식이 버전 복원 후에도 동일해야 한다.",
    "Storage sizing models account for heap tuples, primary-key btree overhead, external TOAST chunks, and dead tuples between autovacuum runs.",
    "다중 조직 격리 검증: PostgreSQL 행 수준 보안 정책 아래에서도 마이그레이션 백필이 모든 조직의 기존 행을 누락 없이 갱신하는지 확인한다.",
    "Observability dashboards track p95 commit latency, table bloat ratio, and unreferenced attachment cleanup throughput over rolling windows.",
    "검토 의견 반영: 설정 변경으로 예산이 낮아지면 즉시 전체 테이블을 잠그지 않고 해당 카드에 다음 판이 기록될 때 점진적으로 정리한다.",
];

/// Builds a ~3.95 MiB realistic `ProseMirror` document (headings, marked prose in English
/// and Korean, lists, task items, blockquotes, and code blocks) where each version edits
/// and adds a subset of sections while staying within `BODY_MAX_NODES` and `BODY_MAX_BYTES`.
fn realistic_doc_version(version: usize) -> Body {
    use std::fmt::Write as _;
    const TARGET_BYTES: usize = 3_950 * 1024;
    let mut rng = SplitMix64(0x1234_5678_9abc_def0);
    let mut content: Vec<Value> = Vec::with_capacity(2000);
    let mut approx_bytes = 32_usize;
    let mut section = 0_usize;
    while approx_bytes < TARGET_BYTES {
        let mut prose = String::with_capacity(2600);
        for step in 0..10_usize {
            let pick = usize::try_from(rng.next_u64() % 16).expect("index");
            let tag = rng.token(10);
            let _ = write!(
                &mut prose,
                "[s{section}.{step}:{tag}] {} ",
                REALISTIC_SENTENCES[pick]
            );
        }
        let mut snippet = String::with_capacity(1400);
        for line in 0..16_usize {
            let token = rng.token(12);
            let rev = if (section + line).is_multiple_of(19) {
                version
            } else {
                1
            };
            let _ = writeln!(
                &mut snippet,
                "let entry_{section}_{line} = compute_budget({section}, {line}, \"{token}\"); // rev {rev}"
            );
        }
        let mut para_content = vec![
            json!({"type": "text", "text": prose}),
            json!({"type": "text", "marks": [{"type": "bold"}], "text": format!("Metric-{section}")}),
            json!({"type": "text", "text": " — "}),
            json!({"type": "text", "marks": [{"type": "code"}], "text": format!("body_versions.sec_{section}")}),
        ];
        if (section + version).is_multiple_of(13) {
            para_content.push(json!({
                "type": "text",
                "text": format!(" [revised in version {version}: updated section {section} notes ({})]", version * 97 + section)
            }));
        }
        let heading = json!({
            "type": "heading",
            "attrs": {"level": (section % 3) + 1},
            "content": [{"type": "text", "text": format!("Section {section}: Storage & History Specification (rev {version})")}]
        });
        let paragraph = json!({"type": "paragraph", "content": para_content});
        let list = json!({
            "type": "bulletList",
            "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [
                    {"type": "text", "marks": [{"type": "italic"}], "text": format!("Summary {section}: ")},
                    {"type": "text", "text": format!("{} {}", REALISTIC_SENTENCES[section % REALISTIC_SENTENCES.len()], REALISTIC_SENTENCES[(section + 7) % REALISTIC_SENTENCES.len()])},
                    {"type": "text", "marks": [{"type": "link", "attrs": {"href": format!("https://example.com/specs/section-{section}")}}], "text": format!(" Spec #{section}")}
                ]}]}
            ]
        });
        let tasks = json!({
            "type": "taskList",
            "content": [
                {"type": "taskItem", "attrs": {"checked": section.is_multiple_of(2)}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": format!("Confirm checkpoint {section} on revision {} — {}", if section % 11 == version % 11 { version } else { 1 }, REALISTIC_SENTENCES[(section + 5) % REALISTIC_SENTENCES.len()])}]}]}
            ]
        });
        let code = json!({
            "type": "codeBlock",
            "attrs": {"language": "rust"},
            "content": [{"type": "text", "text": snippet}]
        });
        for node in [heading, paragraph, list, tasks, code] {
            approx_bytes += serde_json::to_string(&node).expect("node json").len() + 1;
            content.push(node);
            if approx_bytes >= TARGET_BYTES {
                break;
            }
        }
        section += 1;
    }
    let doc = json!({"type": "doc", "content": content});
    Body::parse(doc)
        .expect("realistic 4 MiB body is valid")
        .expect("non-empty body")
}

/// Builds a ~3.95 MiB high-entropy / high-churn `ProseMirror` document where every version
/// contains distinct pseudorandom payloads across paragraphs to stress TOAST compression.
fn high_entropy_doc_version(version: usize) -> Body {
    const TARGET_BYTES: usize = 3_950 * 1024;
    let seed =
        0x51a7_2026_0000_0001_u64.wrapping_add(u64::try_from(version).expect("u64") * 1_000_003);
    let mut rng = SplitMix64(seed);
    let mut content: Vec<Value> = Vec::with_capacity(1100);
    let mut approx_bytes = 32_usize;
    let mut idx = 0_usize;
    while approx_bytes < TARGET_BYTES {
        let payload = rng.token(3800);
        let node = json!({
            "type": "paragraph",
            "content": [{"type": "text", "text": format!("v{version:02}-p{idx:04}-{payload}")}]
        });
        approx_bytes += serde_json::to_string(&node).expect("node json").len() + 1;
        content.push(node);
        idx += 1;
    }
    let doc = json!({"type": "doc", "content": content});
    Body::parse(doc)
        .expect("high-entropy 4 MiB body is valid")
        .expect("non-empty body")
}

/// PostgreSQL storage metrics collected for `body_versions`.
struct PgDiskStats {
    compression: String,
    rows: i64,
    sum_body_bytes: i64,
    sum_octet_length: i64,
    sum_pg_column_size: i64,
    heap_bytes: i64,
    toast_table_bytes: i64,
    toast_index_bytes: i64,
    pg_table_size: i64,
    pg_indexes_size: i64,
    pg_total_relation_size: i64,
}

async fn read_pg_disk_stats(conn: &mut sqlx::PgConnection) -> PgDiskStats {
    let compression: String = sqlx::query_scalar("SHOW default_toast_compression")
        .fetch_one(&mut *conn)
        .await
        .expect("compression setting");
    let (rows, sum_body_bytes, sum_octet_length, sum_pg_column_size): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT COUNT(*)::BIGINT, \
                    COALESCE(SUM(body_bytes), 0)::BIGINT, \
                    COALESCE(SUM(OCTET_LENGTH(body::text)), 0)::BIGINT, \
                    COALESCE(SUM(pg_column_size(body)), 0)::BIGINT \
             FROM body_versions",
        )
        .fetch_one(&mut *conn)
        .await
        .expect("row stats");
    let (
        heap_bytes,
        toast_table_bytes,
        toast_index_bytes,
        pg_table_size,
        pg_indexes_size,
        pg_total_relation_size,
    ): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT pg_relation_size(c.oid)::BIGINT, \
                COALESCE(pg_relation_size(c.reltoastrelid), 0)::BIGINT, \
                COALESCE(pg_indexes_size(c.reltoastrelid), 0)::BIGINT, \
                pg_table_size(c.oid)::BIGINT, \
                pg_indexes_size(c.oid)::BIGINT, \
                pg_total_relation_size(c.oid)::BIGINT \
         FROM pg_class c \
         WHERE c.oid = 'body_versions'::regclass",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("relation sizes");
    PgDiskStats {
        compression,
        rows,
        sum_body_bytes,
        sum_octet_length,
        sum_pg_column_size,
        heap_bytes,
        toast_table_bytes,
        toast_index_bytes,
        pg_table_size,
        pg_indexes_size,
        pg_total_relation_size,
    }
}

fn format_ratio(numer: i64, denom: i64) -> String {
    match (u128::try_from(numer), u128::try_from(denom)) {
        (Ok(n), Ok(d)) if d > 0 => {
            let scaled = (n.saturating_mul(100) + d / 2) / d;
            format!("{}.{:02}x", scaled / 100, scaled % 100)
        }
        _ => "0.00x".to_owned(),
    }
}

async fn run_pg_scenario(
    label: &str,
    max_bytes: u64,
    make_body: fn(usize) -> Body,
) -> (PgDiskStats, PgDiskStats) {
    use std::io::Write as _;

    let test = common::setup().await;
    let store = &test.store;
    store.set_body_versions_kept(DEFAULT_BODY_VERSIONS_KEPT);
    store.set_body_versions_max_bytes(max_bytes);
    let author = common::user(store, "bench.author").await;
    let board_id = board_with_files(store, &author, &[]).await;
    let card_id = card_with(store, &board_id, None, &author).await;
    for version in 1..=50_usize {
        write_body(store, &card_id, Some(make_body(version)), &author).await;
    }

    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    sqlx::raw_sql("VACUUM ANALYZE body_versions")
        .execute(&mut conn)
        .await
        .expect("VACUUM ANALYZE");
    let after_vacuum = read_pg_disk_stats(&mut conn).await;

    sqlx::raw_sql("VACUUM FULL ANALYZE body_versions")
        .execute(&mut conn)
        .await
        .expect("VACUUM FULL ANALYZE");
    let after_vacuum_full = read_pg_disk_stats(&mut conn).await;
    conn.close().await.expect("closed");

    for (phase, stats) in [
        ("VACUUM ANALYZE", &after_vacuum),
        ("VACUUM FULL ANALYZE", &after_vacuum_full),
    ] {
        let ratio_col = format_ratio(stats.sum_body_bytes, stats.sum_pg_column_size);
        let ratio_total = format_ratio(stats.sum_body_bytes, stats.pg_total_relation_size);
        let _ = writeln!(
            std::io::stderr(),
            "MEASURE | {label} | {phase} | toast={} | rows={} | body_bytes={} | octet_len={} | \
             pg_column_size={} | heap={} | toast_tbl={} | toast_idx={} | pg_table_size={} | \
             pg_indexes_size={} | pg_total_relation_size={} | ratio_col={ratio_col} | \
             ratio_total={ratio_total}",
            stats.compression,
            stats.rows,
            stats.sum_body_bytes,
            stats.sum_octet_length,
            stats.sum_pg_column_size,
            stats.heap_bytes,
            stats.toast_table_bytes,
            stats.toast_index_bytes,
            stats.pg_table_size,
            stats.pg_indexes_size,
            stats.pg_total_relation_size,
        );
    }

    test.teardown().await;
    (after_vacuum, after_vacuum_full)
}

/// Reproducible PostgreSQL disk-usage measurement for 4 MiB × 50 `body_versions` on one
/// card across realistic document editing and high-entropy worst-case scenarios, comparing
/// unbounded-bytes (50 kept) vs. `DEFAULT_BODY_VERSIONS_MAX_BYTES` (64 MiB).
#[tokio::test]
#[ignore = "PostgreSQL 4 MiB x 50 disk-usage benchmark; run explicitly with --ignored --nocapture"]
async fn measure_postgres_body_versions_disk_usage() {
    assert_eq!(
        common::db_type(),
        DbType::Postgres,
        "this measurement requires PostgreSQL (run via `cargo xtask test --database postgres`)"
    );
    let (real_before, real_before_full) =
        run_pg_scenario("realistic_before_50", u64::MAX, realistic_doc_version).await;
    let (real_after, real_after_full) = run_pg_scenario(
        "realistic_after_64MiB",
        DEFAULT_BODY_VERSIONS_MAX_BYTES,
        realistic_doc_version,
    )
    .await;
    let (ent_before, ent_before_full) =
        run_pg_scenario("high_entropy_before_50", u64::MAX, high_entropy_doc_version).await;
    let (ent_after, ent_after_full) = run_pg_scenario(
        "high_entropy_after_64MiB",
        DEFAULT_BODY_VERSIONS_MAX_BYTES,
        high_entropy_doc_version,
    )
    .await;

    assert_eq!(real_before.rows, 50, "unbounded realistic keeps 50 rows");
    assert_eq!(ent_before.rows, 50, "unbounded high-entropy keeps 50 rows");
    assert!(
        u64::try_from(real_after.sum_body_bytes).expect("u64") <= DEFAULT_BODY_VERSIONS_MAX_BYTES,
        "realistic total stays within 64 MiB budget"
    );
    assert!(
        u64::try_from(ent_after.sum_body_bytes).expect("u64") <= DEFAULT_BODY_VERSIONS_MAX_BYTES,
        "high-entropy total stays within 64 MiB budget"
    );
    assert!(
        real_after.rows > 1 && real_after.rows < real_before.rows,
        "realistic row count reduced while keeping multiple versions"
    );
    assert!(
        ent_after.rows > 1 && ent_after.rows < ent_before.rows,
        "high-entropy row count reduced while keeping multiple versions"
    );
    assert!(
        real_after_full.pg_total_relation_size < real_before_full.pg_total_relation_size,
        "realistic compacted PostgreSQL relation size reduced by byte budget"
    );
    assert!(
        ent_after_full.pg_total_relation_size < ent_before_full.pg_total_relation_size,
        "high-entropy compacted PostgreSQL relation size reduced by byte budget"
    );
}
