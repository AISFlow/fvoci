# 협업 엔진 비교 초안

고정 조사 SHA: `origin/main` `f3f53c907d27982065984249916e7447dd94ac45` (2026-09-30).
이 문서는 코디네이터가 **유지 vs 교체 결정을 시작**하기 위한 조사 기록이다. 제품 협업 코드를 바꾸지 않았고, 엔진 전환을 권고하지 않는다. Vue tracer(#265/#267/#269 등)에 Yjs/Yrs 대체 경로를 넣지 않는다. #261을 닫지 않는다.

근거는 이 SHA의 제품 코드·테스트·`docs/rewrite.md` §4–§6과 `crates/collab-engine/README.md`다. Automerge·Loro·Y-Sweet 등은 아래 §2에서 범위 밖으로 둔다.

## 1. 현재 제품 경로

서버 스택은 `AGENTS.md`와 `docs/rewrite.md` §4가 고정한 대로 **Rust + Tokio + axum 0.8 + Yrs native helper**다. 협업은 `FVOCI_COLLAB_ENGINE`이 빌드된 `collab-engine` 바이너리를 가리킬 때만 켜진다. 없으면 `/collab`은 `503 collab_unavailable`이다.

### crate·프로세스 경계

| 층 | 위치 | 역할 |
| --- | --- | --- |
| 제품 HTTP/WS | `src/collab/` (`mod.rs`, `transport.rs`, `wire.rs`, `hub.rs`, `room.rs`, `admission.rs`, `awareness.rs`, `y_sync.rs`, `revision.rs`) | Hocuspocus 4.6.0 `/collab`, ACL, persist barrier, room actor FIFO |
| DB 정본 | `src/db/collab.rs` | 인가·snapshot/tail·op receipt·writer fence. `COLLAB_STATE_ENCODING_V1 = 1` |
| 부모 브리지 | `src/collab/engine_bridge.rs` | 전용 std thread. Tokio reactor에서 native 호출을 막지 않음 |
| native child | `crates/collab-engine` (별도 Cargo graph) | Yrs Doc 한 개. WebSocket·DB 없음 |
| Tiptap → Yjs seed | `Request::SeedFromTiptap` + seed child pool(2) | body PUT·duplicate·import. #99 |
| 클라이언트 | React `collab-session.tsx` + Vue `useCollabRoom.ts`, `@fvoci/editor` | 같은 `/collab`·같은 Yjs Doc |

Yrs는 **제품 서버 프로세스 안에서 돌지 않는다.** 부모는 `collab-engine`을 `default-features = false`로 링크하고, feature `worker`가 Yrs와 helper 바이너리를 컴파일한다. room open당 helper 1개(#238). spawn 실패는 다음 `recycle`에서 재시도한다. 거부·불확실 후보는 Yrs undo로 DB를 되돌리지 않고, child를 죽인 뒤 커밋된 바이트를 다시 load한다.

격리(rewrite.md §4가 I1–I5로 인용; 현재 파일은 항목을 나열하지 않음, 구현은 README·`process.rs`): Linux x86_64/aarch64만, `env_clear`, rlimits(`RLIMIT_AS` 1 GiB, 관측 RSS 512 MiB kill), `PR_SET_PDEATHSIG(SIGKILL)`, helper 자체 `oom_score_adj=1000`. rlimits는 자원/실패 격리이지 파일시스템·네트워크 샌드박스가 아니다. 0.28.0은 decode recursion cap이 없어 child rlimit가 상한이다.

### 프로토콜·핀

| 항목 | 이 SHA 값 |
| --- | --- |
| Hocuspocus | 4.6.0 (`@hocuspocus/provider`, React만 `provider-react`). 서버는 자체 codec (`src/collab/wire.rs`) |
| Yrs | `=0.28.0` checksum `52c70dc8…`, feature `small-client`, git rev `23b7f569…` |
| 클라이언트 Yjs | 13.6.32 (`apps/web`, `packages/editor`) |
| 인코딩 | updateV1 / `COLLAB_STATE_ENCODING_V1`. `encoding != 1` → `unsupported/encoding_v2` |
| Doc | `skip_gc=true` (JS `gc: false`), `OffsetKind::Utf16`, fragment `prosemirror` |
| 와이어 금 | `compat/fixtures/hocus-wire.json` (`tests/collab_wire.rs`) |

Hocuspocus MessageType: Sync=0, Awareness=1, Auth=2, QueryAwareness=3, Stateless=5, Close=7, SyncStatus=8, Ping=9, Pong=10. opcode 4/6 없음. Ping은 단일 바이트 `09`, Pong은 `writeVarUint(10)`. 문서 프레임은 `varString(routingKey) + varUint(type) + payload`. inner sync/awareness는 y-protocol이며 Yrs에 그대로 넘기지 않는다. Yrs는 Hocuspocus 서버가 아니다(`compat/README.md`).

helper IPC는 길이 접두 JSON(`u32 LE`): `ping`, `load`, `apply`, `sync`, `snapshot`, `inspect`, `project`, `revision_snapshot`, `restore_from_snapshot`, `replace_from_update`, `seed_from_tiptap`. `applied`는 child Doc뿐이고, parent가 DB commit 뒤에 broadcast한다.

### awareness

`src/collab/awareness.rs`는 Yrs가 아니라 lib0 awareness blob이다. 상한 65 KiB / 클라이언트 128. presence 색·caret `block` id·제목 포커스(`title`)만 싣는다. 세션 JWT를 awareness token으로 쓰지 않는다(토큰은 Yjs `clientID` 십진). generation takeover 후 늦은 Leave는 클리어하지 않는다(#194).

### ACL poll

room actor는 `revoke_poll_ms`(기본 5 s, `FVOCI_COLLAB_REVOKE_POLL_MS`)마다 `poll_acl` → `check_delivery_admission_kind`를 **매 틱** 실행한다. #27 이전에는 한 틱을 건너뛰어 철회가 2배 늦었다. 권한 상실·세션 철회·문서 trash는 다음 poll에서 소켓을 닫는다(`tests/document_collab_lifecycle.rs`, `collab_product` 철회 경로). idle 소켓은 outbound가 없어도 이 poll을 기다린다(`src/db/collab_delivery.rs`).

### room admission

join은 DB `resolve_collab_admission_kind`(멤버십·세션·문서/태스크 권한)와 hub 슬롯을 같이 본다.

- 기본 `FVOCI_COLLAB_MAX_ROOMS=30`(검증 64, 상한 512). 가득 차면 **1013**. 빈 room은 마지막 클라이언트 후 `idle_evict_ms`(기본 30 s)와 reclaim grace(하한 3 s)로 회수(#225).
- `MemoryLedger`: 첫 load 전 예약 RSS + live helper RSS가 `FVOCI_COLLAB_MEMORY_BUDGET`(기본 2 GiB)을 넘으면 거부.
- room마다 PG advisory fence 연결 1개(풀 밖). `max_connections`는 기동 시 `rooms + app pool + reserve 10`으로 검사(#46).
- helper slot 포화는 1013(#238; 이전 1011과 구분).

### snapshot / revision restore

영속 상태는 completeV1 snapshot + tail(최대 64행, 합 32 MiB load). 리비전 캡처는 helper `RevisionSnapshot`(state vector + delete set). 복원은 `RestoreFromSnapshot`이 **전방 updateV1**을 만들고, room actor가 persist한 뒤 broadcast한다. HTTP `replaceLiveCollabContent`는 `ReplaceFromUpdate`. compact-gc 복원은 `gc: false`가 필요하다. 오프라인 캡처는 primary pool headroom 4(`OFFLINE_REVISION_PRIMARY_HEADROOM`).

## 2. 이 문서에서 “비교”가 뜻하는 것

rewrite.md §4의 유지 결정:

> 작은 단일 Rust 서버 … 협업은 Yrs native engine … 프레임워크·CRDT·HWP 구현체를 다시 비교하지 않는다.

§5는 Yrs **교체 후보를 적지 않는다.** 알려진 결함은 caret(#24), ACL tick(#27), room당 PG 연결, helper OOM backstop(#238), room 상한 1013/유휴 지연, `collab_product` 디버그 flake다. §6.1 보류는 `flushDelay` 50/100, DocumentView 구독 분리, Svelte·Astro·Valkey·대규모 room **재설계**이며, 다른 CRDT 엔진이 아니다.

따라서 여기서의 비교는:

1. **기본안: 현재 Yrs 0.28.0 + native child + Hocuspocus 4.6.0 유지.** 전환은 이 SHA에서 재현된, 현재 경로로 고칠 수 없는 차단 결함이 있을 때만 연다.
2. **이미 FVOCI가 글로 남긴 선택지만 비용 항목으로 둔다.** in-process Yrs(격리 포기), helper 수/메모리 예산 조정, Hocuspocus 핀 유지, 대규모 room 재설계 보류.
3. **구현·Vue 삽입이 아니다.** 결정 전에 쓸 계약과 비용을 적는다.

### 범위 밖 (rewrite.md §5/§6에 없음)

Automerge, Loro, Y-Sweet, Diamond, Liveblocks, Jazz, y-websocket 단독 서버 등 **이 저장소의 rewrite.md·candidates.md·collab-engine README에 제품 후보로 적힌 적이 없다.** 표준 구현 스킬 `candidates.md`는 “문서/CRDT는 RFC가 아니라 현재 client 계약”이며 **Yrs를 재사용**하라고만 한다. 라이선스가 MIT여도, 목록에 오르기 전에는 평가하지 않는다.

`compat/` Yrs 0.23.5 브리지는 2026-09-24 위험 probe이지 제품이 아니다. 제품 Yrs는 0.28.0이다. 열린 PR #266이 `compat/` 금본을 `tests/fixtures/`로 옮기려 하므로, 이후 SHA에서는 경로만 바뀌고 계약은 같다.

## 3. 어떤 엔진이든 유지해야 하는 실패·호환 계약

라이브러리 호환 주장이 아니라 FVOCI 재현이다(source-contract 스킬). 이 SHA에서 대표 경로:

| 계약 | 이 SHA의 근거 |
| --- | --- |
| 두 클라이언트 동시 갱신, persist 후 broadcast | `tests/collab_product.rs` `collab_two_clients_update_persists_and_broadcasts` (clientID 11/22) |
| 한글·이모지 updateV1 | `crates/collab-engine/tests/yjs_compat.rs` `korean_emoji_*`, fixture `korean_emoji_base.v1` / `utf8_korean.v1`; seed `compat/fixtures/yjs-seed` `md-g01_korean`·`md-g02_emoji`·`md-02-korean-emoji`; 파생 본문 `tests/fixtures/collab-derived/` |
| 잘못된 UTF-8은 malformed, child recycle | `collab_product` `invalid_utf8_update_candidate` |
| 재접속 Step1이 서버 state vector를 포함 | `collab_reconnect_step1_includes_server_state_vector`; fence 유실 후 재접속 `collab_product` |
| 권한/세션 철회 후 쓰기 거부·소켓 종료 | `collab_append_revoke_*`, `collab_revoked_session_closes_without_post_revoke_broadcast`, `document_collab_lifecycle` trash→ACL poll |
| 리비전 복원 + 재시작 후 이어서 편집 | `RestoreFromSnapshot`; `collab_project_document_persist_projects_body_and_restores_after_restart`; `tests/revision_integration.rs` |
| Hocuspocus 4.6.0 바이트 금 | `tests/collab_wire.rs` ↔ `compat/fixtures/hocus-wire.json` |
| Tiptap JSON seed 트리 동등 | `crates/collab-engine/tests/seed_compat.rs` (68 equal / 3 refused, #99) |
| 브라우저 2UI | `apps/web/e2e-pending/workspace-wiki-collab.spec.ts` (한글·漢字·emoji UniqueID, persist ACK). Vue는 같은 `/collab`을 `useCollabRoom`으로 붙인다(`e2e/workspace-wiki-vue-flow.spec.ts`, `e2e-pending/workspace-wiki-vue-collab.spec.ts`) |
| actor panic/rejoin, helper SIGKILL | `tests/collab_lifecycle.rs` (#114, #131) |

엔진을 바꾸면 위 경로를 **같은 실패 조건으로** 다시 통과해야 한다. skip/mock으로 숨기지 않는다. #266이 열려 있으면 금본 경로만 `tests/fixtures/{collab,yjs-seed,…}`로 옮길 수 있다. 바이트를 재생성해 원본 Node에 맞추지 말 것(#266 본문).

추가 불변식:

- persist barrier: durable commit 전 broadcast 없음. `durable: false`는 child 보고일 뿐이다.
- writer generation / lease: 축출 후 늦은 Leave 무시(#194).
- 빈 byte update 거부(`collab_empty_byte_update_is_rejected`).
- Linux X11 IBus hangul witness는 수락됐으나 Windows·macOS·모바일 IME는 범위 밖(§5). 합성 이벤트를 IME 검증으로 쓰지 않는다.

## 4. 유지 비용 vs 전환 비용

### 유지 (Yrs + native child + Hocuspocus 4.6.0)

이미 수락된 제품이다(기능 대응표 협업 행, #6부터 #238). 남는 운영 비용:

- **encode/decode:** 모든 후보는 child JSON+base64와 updateV1을 지난다. Yrs 0.28.0 `small-client`는 Yjs 13.6.32 32-bit clientID와 맞춰 두었다. Project JSON은 mark 키 정렬 등 JS와 바이트 동일하지 않은 의도적 차이(README).
- **native child:** room당 프로세스·전용 스레드·PG fence 연결. 용량은 개수보다 메모리 예산(#225/#238). 격리 때문에 포기한 비용이지 미구현이 아니다.
- **클라이언트:** React는 `@hocuspocus/provider-react`, Vue는 provider를 composable이 직접 연다(`new HocuspocusProvider`, `gc: false`, `flushDelay: 200`). 둘 다 `collab-model`·`collab-reconnect`·Yjs 13.6.32. 엔진을 바꾸면 **두 host**를 동시에 깨뜨린다. tracer 진행 중에는 특히 금지.
- **e2e:** collaboration-flow, wiki collab pending, Vue wiki flow, task body/archive persist, project document revisions. 전환 시 전부 재실행.
- **저장 데이터:** 기존 `document_states`/`task_states`는 encoding 1 updateV1. 다른 CRDT는 migration·이중 읽기·다운그레이드 거부와 충돌한다.

이 SHA에서 Yrs decode가 제품 협업을 **막는** 재현 결함은 문서화되어 있지 않다. 0.28.0 recursion cap 부재는 child rlimit로 다루고 있다.

### 전환 (다른 엔진 또는 in-process Yrs)

구체적 차단 증거 없이 권고하지 않는다. 가상의 교체가 건드릴 경계만:

- 저장된 본문·리비전 스냅샷·import seed·duplicate.
- `/collab` wire 또는 새 provider (Hocuspocus 클라이언트 전부).
- Tiptap `y-tiptap` / `prosemirror` fragment / UniqueID.
- awareness·persist stateless(`persist:` / `persisted:`).
- helper 격리 재설계(in-process면 신뢰하지 않는 디코더가 앱 역할 프로세스에 들어온다).
- collab_product / lifecycle / wire / seed_compat / 브라우저 2UI.

in-process Yrs는 rewrite.md가 유지하는 격리를 풀어, helper OOM·stack overflow를 서버와 공유한다. 대규모 room 재설계(§6.1)는 엔진 교체가 아니라 용량 모델이다.

## 5. 다음 실험 (가장 작고 되돌릴 수 있는 것)

**구현하지 않는다.** Vue tracer와 #266 fixture 이동이 끝난 뒤에, 코디네이터가 고정 SHA에서 아래를 **기존 테스트만** 묶어 한 번 돌린 결과를 이 문서에 붙이면 충분하다.

계약 팩(새 코드 없음):

1. `cargo test --locked --offline --test collab_wire`
2. `cargo test --locked --offline --manifest-path crates/collab-engine/Cargo.toml --features worker --test yjs_compat --test seed_compat`
3. db-tests: `collab_two_clients_update_persists_and_broadcasts`, 한글 fixture 적용, reconnect Step1, 세션 철회, restore-after-restart (`collab_product` / `collab_lifecycle` / `revision_integration`에서 해당 이름)
4. 브라우저: `workspace-wiki-collab` 한글·emoji 시나리오와 Vue `workspace-wiki-vue-collab` / `workspace-wiki-vue-flow`(이미 있는 spec). **새 Yjs 어댑터 없음.**

이 팩이 깨지지 않는 한 엔진 교체 PR을 열지 않는다. 나중에 격리가 실제 용량 차단이 되면, **제품 `/collab`·Vue와 분리된** throwaway binary로 `yjs_compat` fixture만 in-process Yrs에 돌려 RSS/시간을 재는 실험이 다음 최소 단위다. 그 바이너리를 서버에 넣거나 다른 CRDT를 추가하지 않는다.

코디네이터 결정 기본값: **Yrs 유지.** 이 초안은 #261(Vue 편집기 컨트롤)을 닫지 않는다. 비교는 Vue tracer 이후 작업이며, tracer CI와 병렬로 문서만 둔다.
