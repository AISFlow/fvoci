# FVOCI 0.6.0 — 현재 계획과 수락 경계

이 문서는 승인된 범위, 사용자 흐름별 남은 수락과 다음 소유권의 정본이다.
역할·권한은 [AGENTS.md](../AGENTS.md), 도구·provenance·재사용 조건은
[환경 기록](../.agents/environment.md), 설치·복구는 [RUNNING.md](../RUNNING.md),
발행 절차는 [RELEASING.md](RELEASING.md)를 따른다. 실행 일지는 이슈와 영속 근거에 두고 본문에 누적하지 않는다.

## 1. 목표·승인 범위·현재 후보

0.6.0의 목표는 현재 Vue 3 + Nuxt UI + Tiptap 흐름과 작은 Rust 서버에서
**PostgreSQL·로컬 SQLite·원격 libSQL/Turso**를 끝까지 연결하는 것이다.
세 backend는 모두 승인 범위이며, 승인 자체가 지원 완료를 뜻하지 않는다.
실제 사용자 UI → Rust 인증·현재 리소스 인가 → 선택 backend의 commit → 해당 쓰기와 일치하는 ACK →
새 클라이언트 readback·재시작·현재 archive의 다른 설치본 복원이 수락 단위다.
현재 데이터·리비전·ID·참조·첨부·숫자/시간 정밀도·권한을 보존하고 ON 회귀와 OFF CAS·충돌·초안을 함께 확인한다.

관측 기준: **2026-10-07 08:33 UTC**. 고정 Git·원격 PR 및 ROOT의 현재 근거를 대조했다.
문서 후속 커밋의 SHA를 재귀적으로 기록하지 않으며, 이후 상태는 실제 원격과 이슈를 다시 읽는다.

| 기준                                                         | 고정 값·상태                                                                                                                | 수락 경계                                                                                       |
| ------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| 원본 계약                                                    | main `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`, PR #999 `393795261322b916e588043cf94feca999175843`                         | 별도 읽기 전용 참조; 최신 원본으로 범위 자동 확대하지 않음                                      |
| 대상 main                                                    | `6fb29931bf0af6e41b4bdff2cb039dd8b43df2c5`                                                                                  | 현재 기준선; 0.6 전체 수락 아님                                                                 |
| 제품 [Draft #347](https://github.com/AISFlow/fvoci/pull/347) | head `d6862f15bc39d0864109d7d6e70600602aa26943`, tree `4224b853e35edca53f35ee90e4adc307ba9f89df`, base main6fb              | ROOT의 일반 push·원격 SHA 확인 완료; 고정 조합 SOURCE_ONLY 수락                                 |
| #272 Vue 전환                                                | [MERGED](https://github.com/AISFlow/fvoci/pull/272), 2026-09-30 23:27:28Z, merge `67c3e19ab953169131013bcc2753dfb7ac39229c` | 현재 Vue와 기존 수락 보존. #272 전환·파서 검토·push 대기를 다시 배정하지 않음; 전체0.6은 미완료 |

현재 근거는 [#331](https://github.com/AISFlow/fvoci/issues/331), [OFF #335](https://github.com/AISFlow/fvoci/issues/335),
[설치·현재 데이터 #342](https://github.com/AISFlow/fvoci/issues/342)다.
영속 root `E` = `/home/kinesis/orca/fvoci-evidence/v060-20261004/coherent-final-ci-root/`.
현재 main/head/소유권 관측은 `E/docs-alignment-20261007/current-basis.json`과 같은 디렉터리의 issue snapshots,
재개·WIP 보존은 [종료 전 인계](https://github.com/AISFlow/fvoci/issues/331#issuecomment-6030957827)와
`E/current-ownership-ledger.json`을 따른다. 이전162개 worktree·11개 dirty WIP·실패·Run을 보존했으며 옛 PID는 현재 실행 권한이 아니다.

### 1.1 증거 종류와 실제 상태

**SOURCE**는 diff·계약 검토, **compile**은 빌드, **unit**은 해당 검사 본문,
**real DB**는 실제 backend·역할·트랜잭션, **browser**는 UI, **image**는 실제 산출물,
**full CI**는 고정 head/base/tested merge의 필수 job·aggregate 수락이다. 서로 대신하지 않는다.
PASS·FAIL·CANCELLED·NOTRUN·SKIP·MISSING을 구분한다. 실행 시작·준비·0개 검사·ignored·재시도·조건부 skip을 PASS로 세지 않는다.

| 입력·근거                                                                       | 실제 결과                                                                                                                                     | 재사용 한계·남은 수락                                                                                                                                                                                |
| ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| d686 / `E/d686-integration-fast/`·`E/sqlite-ui-cost-v3-and-composition-review/` | maintained CI선택193·OFF등록16·초기실패13 PASS; 별도 `ACCEPT_FIXED_COMPOSITION_SOURCE_ONLY`                                                   | 첫 직접 Python CI선택 실행의 준비 FAIL을 보존; maintained 명령 PASS와 구분. d686 전체 native/UI/CI 수락 아님                                                                                         |
| SQLite c207 / #331·#335                                                         | native9와 serial child PASS, scoped 검토·통합                                                                                                 | 원래 host FAIL/NOTRUN·bootstrap14 보존; 해당 source/입력의 reader 범위만, d686 UI/ARM/전체 수락 아님                                                                                                 |
| Turso/selected 2c1d / #331                                                      | native-check7 중 현재3 PASS, scoped 검토·통합                                                                                                 | 나머지 검사와 hosted UI 수락 아님; 원 실패와 SOURCE_ONLY 판정 구분                                                                                                                                   |
| SQLite UI/archive/비용 v3 / `E/remaining-sqlite-ui-cost-author/source-v3/`      | 47-member seal `546b61d42141dffc4899945aa5c462d69091ccf16b23fa66a98676be4d5a9680`, 별도 `ACCEPT_SOURCE_ONLY`                                  | 정상 기동 observer 실증 MISSING; v3는 b9에 고정. d686 source/codec/command/native/dist/ABI 입력 재바인딩·실제 SQLite 흐름/측정 NOTRUN                                                                |
| 실제 image source9202 / `E/image9202-postgres-flow-root-acceptance.json`        | 독립 `ACCEPT_ACTUAL_IMAGE_POSTGRES_FLOW_ONLY`: 설치·UI 저장/일치 ACK·앱 역할 DB/native history·새 client·재시작·현재 반출·별도 설치 복원 PASS | image396a의 PostgreSQL 해당 흐름. 중단 없는 단일23-PASS 실행으로 바꾸지 않음. 입력 불변이면 반복하지 않고 새 delta만 확인; SQLite/Turso로 확대하지 않음                                              |
| source9202 실제 Turso / #331 인계                                               | current12/FK·migration·동일 writer/primary readback·rollback·재연결 scoped 수락                                                               | hosted Turso UI NOTRUN. 과거 false 복구와 현재 exact `FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=false` 조회 확인을 보존. 앞선 short alias404는 조회명 오류였으며 설정 변경/hosted 실행 없음                 |
| 설치 수명 수정 / #331 인계·후속                                                 | actual4 PASS/0 ignored/exit0, 15 receipts = 관측 종료12 + 의도적 exceptional-force-reap3                                                      | baseline3 PASS/1 FAIL101·원 Web nested log MISSING 유지. 정상 cancellation/Closed143/noexec·6초 hold·EOF/admission·inode/ledger/user/restart만 수락; 제품 수명 결함 입증 아님                        |
| 이전 b9 전체 CI / #331·#335                                                     | 3 SUCCESS/1 CANCELLED/1 FAILURE                                                                                                               | Rust cache post 취소·Web FAIL 유지; green 아님. 다른 후보 PASS를 합산하지 않음                                                                                                                       |
| 현재 d686 전체 CI                                                               | Rust37593370631·Web37593370517·Install37593370665·Documents37593370705·Collab37593370889 START, 결과 PENDING                                  | 필수 실제 body/gate·head/base/merge 수락 회수 전 전체 PASS 금지. 별도 push-event Turso37593363359 FAIL은 workflow parse/job.env의 runner.temp 후보를 별도 작성자가 조사 중이며 hosted UI 실행이 아님 |

## 2. 구현 원칙과 신규 설치 계약

제품 서버는 Rust stable/Tokio/axum 0.8/Tower/SQLx/Serde/tracing이다. Yrs 협업과 rhwp·문서 parser는 격리 native child로 실행한다.
새 Node 제품 의존·JS fallback·내장 JS 엔진·외부 변환 서비스 우회를 만들지 않는다. Bun/Node 개발 도구·CodeGraph·TS/PDF oracle·브라우저 JS는 제품 서버와 구분한다.
이번 차수에 Yjs/Yrs 교체를 끼워 넣지 않는다. 수락 뒤 승인된 비용·복구·schema 비교를 진행하며 Yrs 유지 결론을 선결정하거나 새 미해결 결함을 비교의 추가 선행으로 만들지 않는다.

기존 공통 gate·최소 backend adapter·단일 제품 권한 정책·검사 catalog/hash authority를 사용한다.
ORM·별도 엔진·오케스트레이터·프레임워크·병렬 정책 정본을 추가하지 않는다. 표준 처리는 유지보수되는 구현에 FVOCI 정책만 얇게 연결한다.
공유 API·manifest/lock·migration 순서·CI·agent 지시는 ROOT 소유 조정 없이 바꾸지 않는다.

권한은 구체적인 제품 연산과 실제 DB에서 재검사한다. 세션 철회·현재 리소스 역할·원자성·잠금 순서·schema gate를 유지한다.
PostgreSQL RLS와 SQLite 파일/remote credential 접근 보안을 동일시하지 않는다. 정상 서버는 제한된 앱 역할/권한으로 실행하며 준비 credential·Meili master key와 분리한다.
검색 필터 뒤 인가된 DB hydrate가 보안 경계다. Meili 부재는 검색503, 키 거부401/403은 기동 실패다.
설치는 사용자 `.env`·`compose.user.yml`와 설정 검증 → DB/migration/grant → 검색 준비 절차이며 정상 컨테이너 서버 uid1000·non-dumpable·기존 cap/secret/격리/종료 계약을 유지한다.

### 2.1 현재 060 계보 — 정확히 12개 도메인 단계

정본은 [실제 등록](../src/db/migrate.rs)과 SQL이다. PostgreSQL 신규 설치는 **`fvoci-postgres-060`**,
로컬 SQLite·원격 libSQL/Turso 신규 설치는 **`fvoci-sqlite-060`**이다. 아래1–12 순서가 두 registry의 실제 실행 순서이며 PG 버전 marker가 아니다.

| 순서 | 도메인        | PostgreSQL                                                          | SQLite family                                                     |
| ---- | ------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------- |
| 01   | core          | [01_core](../migrations/postgres/060/01_core.sql)                   | [01_core](../migrations/sqlite/060/01_core.sql)                   |
| 02   | identity      | [02_identity](../migrations/postgres/060/02_identity.sql)           | [02_identity](../migrations/sqlite/060/02_identity.sql)           |
| 03   | workspaces    | [03_workspaces](../migrations/postgres/060/03_workspaces.sql)       | [03_workspaces](../migrations/sqlite/060/03_workspaces.sql)       |
| 04   | events        | [04_events](../migrations/postgres/060/04_events.sql)               | [04_events](../migrations/sqlite/060/04_events.sql)               |
| 05   | projects      | [05_projects](../migrations/postgres/060/05_projects.sql)           | [05_projects](../migrations/sqlite/060/05_projects.sql)           |
| 06   | documents     | [06_documents](../migrations/postgres/060/06_documents.sql)         | [06_documents](../migrations/sqlite/060/06_documents.sql)         |
| 07   | attachments   | [07_attachments](../migrations/postgres/060/07_attachments.sql)     | [07_attachments](../migrations/sqlite/060/07_attachments.sql)     |
| 08   | collections   | [08_collections](../migrations/postgres/060/08_collections.sql)     | [08_collections](../migrations/sqlite/060/08_collections.sql)     |
| 09   | notifications | [09_notifications](../migrations/postgres/060/09_notifications.sql) | [09_notifications](../migrations/sqlite/060/09_notifications.sql) |
| 10   | integrations  | [10_integrations](../migrations/postgres/060/10_integrations.sql)   | [10_integrations](../migrations/sqlite/060/10_integrations.sql)   |
| 11   | imports       | [11_imports](../migrations/postgres/060/11_imports.sql)             | [11_imports](../migrations/sqlite/060/11_imports.sql)             |
| 12   | operations    | [12_operations](../migrations/postgres/060/12_operations.sql)       | [12_operations](../migrations/sqlite/060/12_operations.sql)       |

빈 DB에서 순서대로 설치하고 `(version, lineage, sql_sha256)`을 exact compiled SQL checksum과 대조한다.
DDL과 receipt는 같은 단계 transaction에서 commit한다. 정확한 prefix만 다음 단계부터 재개하며 gap·foreign lineage·변조 digest·ahead·retired 개발 계보는 무효과 거부한다. 현재 schema/catalog 일치는 각 backend의 gate·실제 catalog 비교로 검증한다.
PG migrate와 [앱 역할 grant](../scripts/grant-app-role.sql)를 구분하고, grant는 원자적으로 적용하며 superuser/BYPASSRLS 역할을 거부한다. SQLite/remote FK·transaction·현재 catalog gate를 별도로 검증한다.
실패 rollback·commit 응답 UNKNOWN·새 연결 receipt·재시작·healthy progress/close/drain은 성공 설치와 함께 수락한다.

**공개 전 폐기 가능한 개발 버전의 제자리 upgrade/downgrade·이전 호환은 0.6 완료 gate가 아니다.**
001–055를 현재 설치 정본으로 되살리지 않는다. 현재 데이터/schema/checksum·실패 migration 재시작·현재 native archive 반출/다른 설치 복원 보장은 유지한다.
backend 간 archive의 지원/거부와 WAL 포함 snapshot·첨부 일관성은 실제 검증으로 명시하고, SQLite main 파일 단독 복사를 backup으로 세지 않는다.

### 2.2 유지하는 UI·프로토콜·출처 계약

현재 Vue 직접 URL·reload/back·encoded slug/ref·query/hash·catch-all/foreign404·역할/오류·저장 재조회가 대상이다.
React 전환을 반복하지 않으며 과거 emitted React0 근거를 유지한다. PDF 개발 oracle는 별도다.
Rust/OpenAPI/cookie·CSP/sanitize·upload·같은 Y.Doc/provider/schema·persist barrier를 보존한다.
UCalendar는 날짜 picker이며 독립 event CRUD가 아니다. IME/focus·개인 undo·Escape/Tab/modal/selection과 Calendar date/datetime/null/DST/version 충돌을 유지한다.
TOTP `totp-rs`, OIDC/JWT `openidconnect`와 SSRF 가드, MCP stdio16MiB cap을 유지하며 rmcp adapter/HTTP transport는 별도 판단이다.
프로필 감사·철회 중 쓰기 차단·429·엄격 ISO 날짜, 원본 project 문서 그룹404와 Rust 미등록404 표면 차이는 기존 승인된 차이다.
`outbox-reset`의 skip 진단·이유 기록은 `--recover-outbox`로 대체하지 않는다. 기존 분리 CLI는 Rust 서버 내부 연산·migrate·backup/restore로 대응하며 seed/mailbox/Python oracle는 제품 기능으로 세지 않는다.
추출 plain text·layout viewer·편집 사본을 구분하며 Office desktop reflow/차트/편집 동등성을 주장하지 않는다.

destination별 source/SHA·변경·전체 라이선스는 [웹 NOTICE](../apps/web/NOTICE.md)와 package 고지가 정본이다.
Nuxt Dashboard `57e8a76e85ac382f2dd75946aa450afb1b3e4b0d`, Editor `60886bda1442549b90312ab5097a449eff634fd1`, Calendar `11809148a32a40612d1d7ddab8aef5372ad46edf`의 MIT와 frontend-design `41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f`의 Apache-2.0 출처를 보존한다.
Nuxt MIT copyright는 `Copyright (c) 2025 Nuxt UI Templates`, LICENSE SHA256은 `e40f408c466e72a3b02eabe846ef35cfab210c14daff782a072dd4903d911f93`다.
manifest/lock 전체 복사·Zod 버전 차이에 따른 제품 schema 변경 없이 dependency·폰트·아이콘 고지와 실제 image HTTP의 full NOTICE를 확인한다.

## 3. 남은 흐름별 수락 단위

각 단위는 세 승인 backend의 **UI → Rust 인가 → 선택 DB commit → matching ACK → 새 client**를 연결한다.
공통 후보는 d686이며 backend/실행 입력을 고정한다. owner 표기는 아래§4의 현재 배정 범위만 뜻한다.
SOURCE/compile/unit PASS로 실제 DB/browser/image 수락을 대신하지 않고 실패·거부·유실·경합까지 검사한다.
이슈의 기존 실제 수락을 먼저 대조하고 입력·조건이 같은 흐름은 반복하지 않는다. 다른 SHA/DB/feature/이미지의 결과를 합산하지 않는다.

### 3.1 인증·로그아웃·철회·계정

사용자 보장: 로그인·계정 변경·logout/revoke 후 접근과 쓰기/room이 현재 세션 권한을 따르고 거부된 쓰기는 무효과다.
담당/후보: ROOT 최종 수락, Grok priority retry는 **b9 고정 driver/plan 준비만** 담당한다.
남은 실제 검사: SQLite/libSQL logout·비밀번호 변경/재설정의 PG-only gate 후보를 실제 AuthService/Axum·HTTP/DB로 확인하고, 브라우저 세션·재접속·철회 경합을 해당 후보에서 연결한다; 현재 NOTRUN이다.
수락/근거/재사용: 현재 권한·원자성·fresh client 거부를 검증하고 [#331](https://github.com/AISFlow/fvoci/issues/331)·`E/grok-security-audit-20261007/`에 기록한다. 기존 auth Rust·Keycloak 범위만 재사용하며 static finding을 수정·해결 또는 서버 데이터 손실로 선언하지 않는다.

### 3.2 개인 입력·프로젝트 문서·멤버 변경

사용자 보장: 개인 task 입력, project/wiki 본문·참조·첨부가 올바른 리소스에 저장되고 멤버 삭제·역할 변경 직후 접근/쓰기·활성 room이 현재 권한을 따른다.
담당/후보: ROOT의 유한 SQLite runtime 후속 작성자 **미배정**, Turso/selected 작성자는 d686 통합 및 자신의2c1d 연결 준비 범위다.
남은 실제 검사: 원 실패 개인 입력·프로젝트 문서·멤버 삭제를 PG/SQLite ON/OFF 및 hosted Turso UI의 실제 commit/ACK/readback·거부 경합에 연결한다; 해당 새 통합 흐름 NOTRUN이다.
수락/근거/재사용: 새 client 본문·ID/ref·권한과 거부 시 데이터/이벤트 무효과를 확인한다. [#331](https://github.com/AISFlow/fvoci/issues/331)·[#335](https://github.com/AISFlow/fvoci/issues/335), 기존9202 PG 흐름은 불변 입력의 범위만 재사용한다.

### 3.3 ON 회귀·OFF CAS·충돌·초안

사용자 보장: ON 협업·개인 undo/IME/재접속 회귀를 보존하고 OFF 저장은 expected head CAS로 경합을 거부한다; 충돌 표시는 최신 head와 일치하며 미확정 초안이 조용히 소실되지 않는다.
담당/후보: ROOT가 d686 runtime 작성자를 배정하고 정책/결함 판정을 소유한다. Grok retry는 b9 읽기·준비 범위다.
남은 실제 검사: OFF stale CAS·동시 client·권한 철회·저장 실패/재접속·draft retire를 actual HTTP/DB/browser로 연결한다. dirty retire 삭제 기대는 정책 미확정이며 문서로 fix/resolve하지 않는다.
수락/근거/재사용: `E/grok-security-audit-20261007/`의 실제 OffWikiDraft 기존15 PASS·최소 반례1 PASS/2 FAIL을 보존한다. synthetic 클래스 결과와 server CAS source의 head 보호를 구분한다. [#335](https://github.com/AISFlow/fvoci/issues/335)의 기존 ON/OFF 근거만 해당 입력에서 재사용한다.

### 3.4 응답 유실·중복 command·과거 ACK

사용자 보장: lost response는 durable 여부 UNKNOWN으로 처리하고 같은 command 재시도는 중복 mutation을 만들지 않는다; 오래된 receipt가 새 head/충돌을 확정 저장으로 지우지 않는다.
담당/후보: ROOT가 d686의 실제 flow·정책을 수락하며 Turso/selected 작성자는 원격 observer/ACK 연결 준비를 담당한다.
남은 실제 검사: commit 직후 응답 유실·duplicate/retry·historical ACK·fresh readback·권한 변경을 DB/browser에서 연결한다; 현재 새 통합 runtime NOTRUN이다.
수락/근거/재사용: command/revision 식별과 matching ACK·head/충돌 표시·DB 단일 효과를 대조한다. Grok의 real client 클래스 historical receipt 충돌-clearing FAIL과 server CAS source 보호를 함께 보존하되 서버 덮어쓰기 확정으로 확대하지 않는다. [#335](https://github.com/AISFlow/fvoci/issues/335)·`E/grok-security-audit-20261007/findings.md`, Turso migration COMMIT UNKNOWN 증거는 그 migration 범위만 재사용한다.

### 3.5 현재 데이터·리비전·ID·참조·첨부·정밀도

사용자 보장: 현재 본문·revision/UUIDv7·범위 번호·리소스 참조·첨부 bytes/권한·JSON 숫자/시간 의미가 저장·새 client·복원 후 유지된다.
담당/후보: ROOT, SQLite 후속 owner 미배정; candidate d686/native 재바인딩 전 v3는 b9 source-only다.
남은 실제 검사: personal/project/task-origin·revision/history/native reader·첨부참조와 컬렉션 숫자/날짜 정밀도를 세 backend에서 검증한다. c207 native9·2c1d native3는 해당 qualified scope PASS이며 UI/전체 정밀도 수락 아님.
수락/근거/재사용: [#342](https://github.com/AISFlow/fvoci/issues/342)·`E/remaining-sqlite-ui-cost-author/source-v3/`·`E/sqlite-ui-cost-v3-and-composition-review/`의 exact oracle/거부 controls를 유지한다. locale/TZif·codec/ABI·source/command 입력을 고정하고 차이를 정규화로 숨기지 않는다. 현재 archive reader의 tamper/missing 거부·원 실패/봉인은 보존한다.

### 3.6 재시작·반출·별도 설치본 복원

사용자 보장: durable 저장은 정상/실패 후 재시작에도 남으며 현재 archive를 다른 격리 설치본에 복원한 뒤 새 client/native reader가 현재 데이터·권한·리비전·참조·첨부를 확인한다.
담당/후보: ROOT의 SQLite 유한 runtime owner 미배정, Turso 작성자는 hosted 연결 prerequisite 준비; d686 final input은 재바인딩 필요하다.
남은 실제 검사: SQLite 정상 기동 observer 실증 → 실제 UI 저장/ON-OFF → restart → 현재 반출/별도 설치 restore → restart/fresh client; hosted Turso의 해당 UI 흐름은 NOTRUN이다.
수락/근거/재사용: [#342](https://github.com/AISFlow/fvoci/issues/342), `E/image9202-postgres-flow-root-acceptance.json`, `E/remaining-sqlite-ui-cost-author/source-v3/`. 이미 독립 수락한9202 PostgreSQL 흐름은 입력 불변이면 반복하지 않는다. image/compile/readback 범위를 SQLite/Turso에 전용하지 않고 현재 schema12·rollback/close/drain·failure cleanup을 실제 확인한다.

### 3.7 동등 작업량 ON/OFF 자원 비교

사용자 보장: 같은 사용자 작업·저장/ACK·실패 조건에서 ON/OFF 비용을 비교하며 기능/검사를 줄여 더 빠르다고 하지 않는다.
담당/후보: ROOT가 유한 실행 owner·caps를 배정; v3 source-only/b9 고정, d686 정상 기동/측정은 NOTRUN이다.
남은 실제 검사: 같은 backend/data/이미지·작업량·준비/warm 조건을 고정하고 CPU/RSS·ACK latency·DB/room 자원과 정상 종료를 측정한다. non-dumpable 제품 보호를 유지하고 권한 확대 없이 observer/sample 권한을 입증한다.
수락/근거/재사용: `E/remaining-sqlite-ui-cost-author/source-v3/`의 sealed pair-policy/sampler·정상 launch receipt·raw samples·input hash로 판정한다. 순수 controls PASS·다른 환경 성능은 실제 equal-work 비용 수락이 아니다. 남은 측정은 [#335](https://github.com/AISFlow/fvoci/issues/335)에 연결한다.

## 4. 현재 소유권·다음 행동·승인

| 담당                           | 현재 권한·후보                                                                  | 다음 수락                                                                                                                 |
| ------------------------------ | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| ROOT / 기존 Run generation5    | 실제 Sol6.1/high, term_bd7b2ba1-4cf9-4d30-81fb-e7131e14c815                     | d686 CI 회수/실패 분류·통합·현재 owner 확인·SQLite 유한 runtime 배정·최종 수락                                            |
| Turso/selected 작성자          | task_5abeb0def02e / ctx_2aded24d97da, source2c1d·final hosted prerequisite 준비 | 추가 heavy grant 없음; 고정 source/actual native3 범위 유지, hosted UI 준비와 정상 종료/secret 경계 확인                  |
| Grok priority retry            | task_5753a9223979 / ctx_503ea36f4fc9, b9 고정 driver/plan 준비                  | 사용자 명시 retry. 실제 Sol 검토 content unavailable 보존; ROOT 입력/caps/격리 배정 전 build/HTTP/DB 금지                 |
| SQLite UI/archive/비용 runtime | **후속 owner 미배정**, ROOT 책임                                                | source-v3 제출/검토 작성자는 완료·release된 이력이며 active owner로 표시하지 않음; 새 후보 입력/정상 launch를 먼저 바인딩 |

상세 task/dispatch·hash·PID·명령·실패·잔존 자원은 E와 이슈에서 찾는다. 예전 #342의 PID·Fable/Grok/Sol 배정은 당시 사실이며 현재 권한이 아니다.
기존 제품/감사 작성자와11 dirty WIP·데이터·부분 결과·sealed evidence를 보존한다. 문서 정렬을 이유로 기존 실행을 reset/rerun/cleanup/restart하거나 종료된 체인을 재가동하지 않는다.

다음 순서: 현재 d686 필수 CI·원 Turso push-event 실패 회수 → 관련 실패의 최소 delta와 고정 독립 검토 →
SQLite 후속 owner/유한 배정·native/dist/codec/ABI·정상 launch proof → 미실행 사용자 흐름/비용 →
해당 final SHA의 실제 시스템·CI·별도 컨텍스트 검토 수락 → 사용자에게 수락 범위와 미결 정책/한계 보고.
이미 수락한 범위를 다시 구현하거나 입력 불변의9202 PG flow를 반복하지 않는다. 독립 검토와 CI는 병렬 가능하나 둘 다 수락해야 한다.
문서 변경은 문서 검사만 수행하며 기존 CI 선택 정책·gate를 바꾸거나 제품 build를 추가하지 않는다.

현재 ROOT의 일반 제품 브랜치 push·PR 갱신은 승인 범위다. **main 병합(#353/#360/#361 포함)·태그·릴리스·배포·추가 비용·권한 확대는 별도 사용자 승인**이다.
과거 포괄 merge/0.x 발행 승인은 현재 권한으로 자동 적용하지 않는다. 원본 원격 쓰기·main 직접 push·force push·보호 우회·운영 DB/secret/공개 범위 변경 권한을 만들지 않는다.
전체0.6 완료는 세 승인 backend의 기능/UI·보안·데이터/복구·필수 산출물/검사·고정 독립 검토·승인된 main 수락이 충족돼야 한다.
개별 PR/worker/source 검토·compile·단위 성공은 전체 종료·발행·배포가 아니다. 미구현·미연결·부분 검증·원본 미제공을 구분하며 opt-in/후속 분류로 필수 범위를 제외하지 않는다.

## 5. 미결 정책·지원 한계·로드맵

Grok A/B/C/D의20영역 **b9 SOURCE 표본 감사**는 회수됐으며 `E/grok-security-audit-20261007/audit-summary.md`,
`findings.md`, `coverage-20.md`, 네 `cohort-*-report.md`와 primary-source supplement가 정본이다.
현재 d686 delta는 b9 감사 범위 밖이며, 감사 완료를 보안·runtime 수락으로 쓰지 않는다.
실제 OFF 클래스15 PASS·반례1 PASS/2 FAIL, dirty retire 정책 미확정·historical receipt UI conflict-clearing,
SQLite/libSQL logout PG gate static 후보·actual HTTP/DB NOTRUN을 유지한다. 문서로 결함 수정·정책 확정·issue 종결을 선언하지 않는다.

### 5.1 보존하는 정책·운영 한계

- ACC-1: OIDC 초대 이메일 일치·검증 요구는 미결정이다. Naver/Kakao email_verified 차이와 원본의 기존 요구 수준을 보존한다. Share B: 링크 생성자 권한 상실 뒤 링크 유지 정책은 미결정이며 생성 시 하위 View 검사 수락·#149 전송 A/B 승인과 구분한다.
- team SSO GET start login CSRF·SSE750ms polling·IPv6 회전/계정별 login 제한은 정책/설계 항목이다. limiter는 process-local/direct socket IP이며 trusted proxy·분산 제한·namespace 축출 한계가 남는다.
- PATCH dueAt은 browser millisecond 비교여서 같은 ms의 sub-ms 충돌을 검출하지 않는다. Gantt DST 막대 하루 차이/overlap rail과 새 Calendar version 계약을 구분한다. 숫자/날짜 정밀도 소스 후보는 실제 검증 전 해결하지 않는다.
- presigned part900초/download60초는 철회·mode전환·삭제 뒤 만료까지 유효하다. 게시 전 권한 재검사·S3 key회전/NTP·HAR 서명 URL 노출·오류 메시지 한계를 유지한다. 운영 지정 GitHub API host DNS pin 차이·remote DB error log 후보를 새 실제 침해로 단정하지 않는다.
- room fence PG 연결·pool reserve10/maintenance claim·receipt/event/audit 보존 정책 부재, import 큰 요청 버퍼/동시 admission, HWP 대기+실행 timeout 최대 약2배를 유지한다. parser child의 rlimit/envclear/종료/non-dumpable 보호를 파일/네트워크 namespace sandbox로 주장하지 않는다.
- 메일 수신자는 최근64 event process memory라 재시작/lease 인계 재발송 가능하며 digest15분/실패 streak가 다음 날 반복 종료돼 뒤 수신자를 굶길 수 있다. SQLx0.8.6 BEGIN 취소는 기존 acquire 검사로 막으며 sqlx0.9/acquire 제거·try_acquire/try_begin은 승인 없이 적용하지 않는다.
- 긴 purge의 xmin/SSE/outbox 지연·SSE64 poll·Meili chunk backlog/처리량·memory ledger mutex 안 /proc 조회·AppArmor OOM backstop 재측정 없음은 측정 한계다. non-dumpable과 sampler 충돌은 권한을 넓혀 우회하지 않는다.
- 설치 보안·복귀는 RUNNING.md가 정본이다. 환경값을 Docker 사용자/root/exec가 읽을 수 있음·정상 서버 준비 credential 부재·같은 uid helper 파일/검색키 공유·PG log_statement>=ddl의 CREATE ROLE PASSWORD 노출을 유지한다. 옛0.1→0.2 schema044 downgrade 거부·0.2→0.3 FILE/backup/미완료 presigned 복귀 위험은 역사적 한계이며 현재 신규 설치 upgrade gate가 아니다.
- BOOT-NETWORK 빈 화면의 원인 UNKNOWN, #237 완화/앱 chunk retry 없음·과거 reconnect afterDestroy 실패/후속 dispose 수락·Dependabot Bun lockfileVersion2/compose recreate 오류는 고정 역사 근거를 보존하고 관련 새 delta에서만 대조한다.

### 5.2 외부 witness와 범위

| 범위                   | 기존 수락·승인                                                                  | 보존하는 잔여/한계                                                                                                |
| ---------------------- | ------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| #149 첨부 A/B          | A/B 모두 승인·#254 Rust·local MinIO/PG·Chromium cross-origin                    | 실제 AWS/다른 S3/CDN/proxy·FF/Safari·clock skew는 기존 F. 증거/사용자 한계 승인 전 제외하지 않음                  |
| OIDC/MFA·workspace SSO | local Keycloak26.7.4 기존 witness·Rust entitlement·QR 구조·callback/mix-up 거부 | 실제 외부 IdP/HTTPS/proxy/container·다른 browser·인증앱 scan, 게시 trust key 부재로 enterprise SSO 활성 지원 불가 |
| OS IME·touch·browser   | Linux X11/IBus hangul2벌식·Chromium/XTest·multirange 선택/ACK/reload/undo       | Windows/macOS/mobile·물리 keyboard/touch·FF/WebKit 밖; CDP/합성을 OS IME 수락으로 쓰지 않음                       |
| Web Push/메일/GitHub   | Chrome153/Linux/FCM1회 subscription/send/SW receive/logout                      | 다른 browser/provider·실제 SMTP/GitHub App·licensedSSO/clipboard/multipage 미실행 유지                            |
| 이미지·설치·복구       | 기존 amd64/ARM 고정쌍·local S3/versioned rollback·현재9202 PG 범위              | 현재 final SHA/ARM S3·Mac Docker Desktop/rootless/Podman 범위 구분; 과거 개발 upgrade는 현재 gate에서 제외        |

외부 witness 잔여를 수락된 Rust 연산의 미구현으로 되돌리지 않는다. 기존 비차단 platform 한계를 새 필수 선행으로 만들지 않고 기존 F도 근거 없이 제외하지 않는다.

### 5.3 로드맵

**0.6.0 필수**는 위 세 backend·ON/OFF 사용자 흐름·현재 신규 설치12단계·현재 데이터/반출복원·동등 작업량 비용·최종 검토/CI 수락이다.
**0.6.x**에 Storybook/Playground·CJK/frontend 정리를 둔다. **공개 베타는0.7.0**이며0.6 완료의 새 필수 조건이 아니다.
#280 협업 엔진 비용·복구/schema 비교와 승인된 썸네일 검토는 기존 선행 뒤 별도 scope로 진행한다. 기본 엔진 채택·사용자 데이터 이전은 비교 뒤 별도 결정이다.
TS 데이터 이전은 기존 사용자 확인으로 현재 범위 밖이며 HWP 썸네일·MCP HTTP·requeue 운영 API/UI·license issuer trust 등 원본 미제공을 자동 신규 구현으로 만들지 않는다.
#266 fixture/probe·#263 docs, recovered digest/import/search/streams/collab/deps WIP와 dependency 후보는 현재 실제 상태/고유 diff를 확인해 별도 소유권으로 이어간다. 완료됐던 작업을 자동 재가동하지 않는다.
mail 영속화/digest·import admission·DB/room/search/outbox/attachment 비용·DTO/권한 join·Prometheus·restore.env quoting·flushDelay/구독 분리 등 후속은 기존 근거와 연결하며 이번 문서로 구현/검사 완료를 선언하지 않는다.

## 6. 역사와 재개 포인터

[고정 d686의 정리 전 본문](https://github.com/AISFlow/fvoci/blob/d6862f15bc39d0864109d7d6e70600602aa26943/docs/rewrite.md)은
#272 당시21개 Rust 기능 대응표·Vue FE ID·lint10범위·파서 교정/입력/이미지/원격 CI의 수락·실패·정책·후속을 보존한다.
[고정 f442의 이전 문서](https://github.com/AISFlow/fvoci/blob/f442a9f06c438b51524e13cb7a2043ff5d95566a/docs/rewrite.md)와
`git log -p -- docs/rewrite.md`도 역사 정본이다. #272 당시 OPEN/Draft·옛 SHA·검토/push 대기는 현재 TODO가 아니다.
본문 압축은 기존 수락·실패·seal·검토·정책 결정을 삭제하거나 새 성공으로 바꾸지 않는다. 새 대형 progresslog/history 파일을 만들지 않는다.

과거 root `H` = `/home/kinesis/orca/fvoci-evidence/recovery-20260930/takeover-evidence/`.
`H/sol-coordinator-docs-20261001/`의 coverage/SHA256SUMS·원 dispatch/spec·구현/검토 보고서,
`H/open-pr-takeover-raw/`·`source-pr-superseded-48f6246c.json`, recovery patch/bundle·runtime-map/cleanup receipt를 그대로 보존한다.
원본 PR closed/superseded는 merge가 아니며 기존 #272 승계와 구분한다. i18n1172a73a는 #165의 같은 제품 코드로 기종결이며 다시 구현하지 않는다.
과거 실패 f442 Web·181 source regex·692 Wiki digit loss/출력 불일치·4b Closed outbound 수정/17browser·native411입력/5binary·1820부분 inventory/1822 NUL-safe 정정·OS 첫 조합 expected-failure/exit1·image4b 재사용 한계는 고정 문서§1/§8 및 `f272-flow-schema-*` 보고서에서 읽는다.
디자인/template/NOTICE 근거는 `front272-official-templates/`, `H/design-b99700ab/`, `H/auth-and-editor-589a7db1/`, `H/ime-multirange-40f090d1/`, `front272-batch9f05/first-composition-bb5aa42b/`와 원 고지에 있다.
과거 v0.3.0·main50d95df1 gate·image digest 및 모델/Astra/Fable/Opus/Grok/Sol·수량 제한·실제 CLI 업데이트 사건은 고정 문서와 [환경 기록§6](../.agents/environment.md#6-과거-기록과-재개-포인터)에 있다. 현재0.6 발행/소유권으로 전용하지 않는다.

재개 순서: AGENTS → 환경 기록의 현재 도구/전체 입력 재사용 규칙 → 본문§1/§3/§4 → 실제 Git·Run/task/dispatch·미처리 질문·CI.
현재 source·실제 실행 SHA·수락 범위와 현재 owner를 분리하고 이미 회수한 근거를 재사용한다.
세션 한계에는 기존 E/이슈에 고정 SHA·미수락 diff·실패·다음 명령·CI·소유 자원을 남긴다. 수락 전 강제 삭제·광범위 kill/prune·다른 Run reset·설정되지 않은 background 실행 약속은 하지 않는다.
