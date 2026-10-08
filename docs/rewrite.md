# FVOCI 0.6.0 — 현재 계획과 수락 경계

이 문서는 승인된 범위, 사용자 흐름별 남은 수락과 다음 소유권의 정본이다.
역할·권한은 [AGENTS.md](../AGENTS.md), 설치·복구는 [RUNNING.md](../RUNNING.md),
발행 절차는 [RELEASING.md](RELEASING.md)를 따른다. 현재 head·CI·수락 상태는 [PR #347](https://github.com/AISFlow/fvoci/pull/347) 본문 체크포인트와
[#331](https://github.com/AISFlow/fvoci/issues/331)이 정본이다. 이 문서에는 현재 SHA나 실행 일지를 적지 않으며 과거 기록은 `git log -p`와 #331 아카이브 댓글에 있다.

## 1. 목표·승인 범위

0.6.0의 목표는 현재 Vue 3 + Nuxt UI + Tiptap 흐름과 작은 Rust 서버에서
**PostgreSQL·로컬 SQLite·원격 libSQL/Turso**를 끝까지 연결하는 것이다.
세 backend는 모두 승인 범위이며, 승인 자체가 지원 완료를 뜻하지 않는다.
실제 사용자 UI → Rust 인증·현재 리소스 인가 → 선택 backend의 commit → 해당 쓰기와 일치하는 ACK →
새 클라이언트 readback·재시작·현재 archive의 다른 설치본 복원이 수락 단위다.
현재 데이터·리비전·ID·참조·첨부·숫자/시간 정밀도·권한을 보존하고 ON 회귀와 OFF CAS·충돌·초안을 함께 확인한다.

원본 계약 기준은 main `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`, PR #999 `393795261322b916e588043cf94feca999175843`이며 별도 읽기 전용 참조다. 최신 원본으로 범위를 자동 확대하지 않는다.
#272 Vue 전환은 main `67c3e19ab953169131013bcc2753dfb7ac39229c`에 머지됐다. 현재 Vue와 기존 수락을 보존하며 #272 작업을 다시 배정하지 않는다.
관련 이슈는 [#331](https://github.com/AISFlow/fvoci/issues/331), [OFF #335](https://github.com/AISFlow/fvoci/issues/335),
[설치·현재 데이터 #342](https://github.com/AISFlow/fvoci/issues/342)다.

### 1.1 증거 종류

**SOURCE**는 diff·계약 검토, **compile**은 빌드, **unit**은 해당 검사 본문,
**real DB**는 실제 backend·역할·트랜잭션, **browser**는 UI, **image**는 실제 산출물,
**full CI**는 고정 head/base/tested merge의 필수 job·aggregate 수락이다. 서로 대신하지 않는다.
PASS·FAIL·CANCELLED·NOTRUN·SKIP·MISSING을 구분한다. 실행 시작·준비·0개 검사·ignored·재시도·조건부 skip을 PASS로 세지 않는다.

실제 결과·수락은 PR #347 체크포인트와 #331이 정본이다. 폐기된 로컬 evidence root에만 근거가 있던 수락은 현재 head에서 다시 검증하기 전에는 쓰지 않는다. 아래 절의 기존 PASS 보존·반복 생략도 #331이나 PR 체크포인트에 근거가 남은 수락에만 적용한다.

## 2. 구현 원칙과 신규 설치 계약

제품 서버는 Rust stable/Tokio/axum 0.8/Tower/SQLx/Serde/tracing이다. Yrs 협업과 rhwp·문서 parser는 격리 native child로 실행한다.
새 Node 제품 의존·JS fallback·내장 JS 엔진·외부 변환 서비스 우회를 만들지 않는다. Bun/Node 개발 도구·CodeGraph·TS/PDF oracle·브라우저 JS는 제품 서버와 구분한다.
이번 차수에 Yjs/Yrs 교체를 끼워 넣지 않는다. 수락 뒤 승인된 비용·복구·schema 비교를 진행하며 Yrs 유지 결론을 선결정하거나 새 미해결 결함을 비교의 추가 선행으로 만들지 않는다.

기존 공통 gate·최소 backend adapter·단일 제품 권한 정책·검사 catalog/hash authority를 사용한다.
ORM·별도 엔진·오케스트레이터·프레임워크·병렬 정책 정본을 추가하지 않는다. 표준 처리는 유지보수되는 구현에 FVOCI 정책만 얇게 연결한다.
공유 API·manifest/lock·migration 순서·CI·agent 지시는 리드가 범위를 정하기 전에는 바꾸지 않는다.

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
backend/실행 입력을 고정한다.
SOURCE/compile/unit PASS로 실제 DB/browser/image 수락을 대신하지 않고 실패·거부·유실·경합까지 검사한다.
이슈의 기존 실제 수락을 먼저 대조하고 입력·조건이 같은 흐름은 반복하지 않는다. 다른 SHA/DB/feature/이미지의 결과를 합산하지 않는다.

### 3.1 인증·로그아웃·철회·계정

사용자 보장: 로그인·계정 변경·logout/revoke 후 접근과 쓰기/room이 현재 세션 권한을 따르고 거부된 쓰기는 무효과다.
남은 실제 검사: SQLite/libSQL logout·비밀번호 변경/재설정의 PG-only gate 후보를 실제 AuthService/Axum·HTTP/DB로 확인하고, 브라우저 세션·재접속·철회 경합을 해당 후보에서 연결한다; 현재 NOTRUN이다.
수락/근거/재사용: 현재 권한·원자성·fresh client 거부를 검증하고 [#331](https://github.com/AISFlow/fvoci/issues/331)에 기록한다. 기존 auth Rust·Keycloak 범위만 재사용하며 static finding을 수정·해결 또는 서버 데이터 손실로 선언하지 않는다.

### 3.2 개인 입력·프로젝트 문서·멤버 변경

사용자 보장: 개인 task 입력, project/wiki 본문·참조·첨부가 올바른 리소스에 저장되고 멤버 삭제·역할 변경 직후 접근/쓰기·활성 room이 현재 권한을 따른다.
남은 실제 검사: 원 실패 개인 입력·프로젝트 문서·멤버 삭제를 PG/SQLite ON/OFF 및 hosted Turso UI의 실제 commit/ACK/readback·거부 경합에 연결한다; 해당 새 통합 흐름 NOTRUN이다.
수락/근거/재사용: 새 client 본문·ID/ref·권한과 거부 시 데이터/이벤트 무효과를 확인한다. [#331](https://github.com/AISFlow/fvoci/issues/331)·[#335](https://github.com/AISFlow/fvoci/issues/335), 기존 PG 흐름 수락은 불변 입력의 범위만 재사용한다.

### 3.3 ON 회귀·OFF CAS·충돌·초안

사용자 보장: ON 협업·개인 undo/IME/재접속 회귀를 보존하고 OFF 저장은 expected head CAS로 경합을 거부한다; 충돌 표시는 최신 head와 일치하며 미확정 초안이 조용히 소실되지 않는다.
남은 실제 검사: OFF stale CAS·동시 client·권한 철회·저장 실패/재접속·draft retire를 actual HTTP/DB/browser로 연결한다. dirty retire 삭제 기대는 정책 미확정이며 문서로 fix/resolve하지 않는다.
수락/근거/재사용: 실제 OffWikiDraft 기존15 PASS·최소 반례1 PASS/2 FAIL을 보존한다. synthetic 클래스 결과와 server CAS source의 head 보호를 구분한다. [#335](https://github.com/AISFlow/fvoci/issues/335)의 기존 ON/OFF 근거만 해당 입력에서 재사용한다.

### 3.4 응답 유실·중복 command·과거 ACK

사용자 보장: lost response는 durable 여부 UNKNOWN으로 처리하고 같은 command 재시도는 중복 mutation을 만들지 않는다; 오래된 receipt가 새 head/충돌을 확정 저장으로 지우지 않는다.
남은 실제 검사: commit 직후 응답 유실·duplicate/retry·historical ACK·fresh readback·권한 변경을 DB/browser에서 연결한다; 현재 새 통합 runtime NOTRUN이다.
수락/근거/재사용: command/revision 식별과 matching ACK·head/충돌 표시·DB 단일 효과를 대조한다. Grok의 real client 클래스 historical receipt 충돌-clearing FAIL과 server CAS source 보호를 함께 보존하되 서버 덮어쓰기 확정으로 확대하지 않는다. [#335](https://github.com/AISFlow/fvoci/issues/335)에 기록하며 Turso migration COMMIT UNKNOWN 증거는 그 migration 범위만 재사용한다.

### 3.5 현재 데이터·리비전·ID·참조·첨부·정밀도

사용자 보장: 현재 본문·revision/UUIDv7·범위 번호·리소스 참조·첨부 bytes/권한·JSON 숫자/시간 의미가 저장·새 client·복원 후 유지된다.
남은 실제 검사: personal/project/task-origin·revision/history/native reader·첨부참조와 컬렉션 숫자/날짜 정밀도를 세 backend에서 검증한다.
수락/근거/재사용: [#342](https://github.com/AISFlow/fvoci/issues/342)의 exact oracle/거부 controls를 유지한다. locale/TZif·codec/ABI·source/command 입력을 고정하고 차이를 정규화로 숨기지 않는다. 현재 archive reader의 tamper/missing 거부·원 실패/봉인은 보존한다.

### 3.6 재시작·반출·별도 설치본 복원

사용자 보장: durable 저장은 정상/실패 후 재시작에도 남으며 현재 archive를 다른 격리 설치본에 복원한 뒤 새 client/native reader가 현재 데이터·권한·리비전·참조·첨부를 확인한다.
남은 실제 검사: SQLite 정상 기동 observer 실증 → 실제 UI 저장/ON-OFF → restart → 현재 반출/별도 설치 restore → restart/fresh client; hosted Turso의 해당 UI 흐름은 NOTRUN이다.
수락/근거/재사용: [#342](https://github.com/AISFlow/fvoci/issues/342). 이미 독립 수락한 PostgreSQL image 흐름은 입력 불변이면 반복하지 않는다. image/compile/readback 범위를 SQLite/Turso에 전용하지 않고 현재 schema12·rollback/close/drain·failure cleanup을 실제 확인한다.

### 3.7 동등 작업량 ON/OFF 자원 비교

사용자 보장: 같은 사용자 작업·저장/ACK·실패 조건에서 ON/OFF 비용을 비교하며 기능/검사를 줄여 더 빠르다고 하지 않는다.
남은 실제 검사: 같은 backend/data/이미지·작업량·준비/warm 조건을 고정하고 CPU/RSS·ACK latency·DB/room 자원과 정상 종료를 측정한다. non-dumpable 제품 보호를 유지하고 권한 확대 없이 observer/sample 권한을 입증한다.
수락/근거/재사용: sealed pair-policy/sampler·정상 launch receipt·raw samples·input hash로 판정한다. 순수 controls PASS·다른 환경 성능은 실제 equal-work 비용 수락이 아니다. 남은 측정은 [#335](https://github.com/AISFlow/fvoci/issues/335)에 연결한다.

## 4. 현재 소유권·다음 행동·승인

현재 담당·다음 작업은 PR #347 본문 체크포인트와 #331에서 읽는다. 이 문서에 담당자·작업 ID를 적지 않는다.
이미 수락한 범위를 다시 구현하지 않는다. 독립 검토와 CI는 병렬 가능하나 둘 다 수락해야 한다.
문서 변경은 문서 검사만 수행하며 기존 CI 선택 정책·gate를 바꾸거나 제품 build를 추가하지 않는다.

AGENTS.md의 통합 역할이 하는 PR 작업 브랜치 일반 push·Draft PR 갱신은 승인 범위다. **main 병합(#353/#360/#361 포함)·태그·릴리스·배포·추가 비용·권한 확대는 별도 사용자 승인**이다.
과거 포괄 merge/0.x 발행 승인은 현재 권한으로 자동 적용하지 않는다. 원본 원격 쓰기·main 직접 push·force push·보호 우회·운영 DB/secret/공개 범위 변경 권한을 만들지 않는다.
전체0.6 완료는 세 승인 backend의 기능/UI·보안·데이터/복구·필수 산출물/검사·고정 독립 검토·승인된 main 수락이 충족돼야 한다.
개별 PR/worker/source 검토·compile·단위 성공은 전체 종료·발행·배포가 아니다. 미구현·미연결·부분 검증·원본 미제공을 구분하며 opt-in/후속 분류로 필수 범위를 제외하지 않는다.

## 5. 미결 정책·지원 한계·로드맵

Grok A/B/C/D의20영역 보안 감사는 **SOURCE 표본 감사**이며 보안·runtime 수락으로 쓰지 않는다.
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
| 이미지·설치·복구       | 기존 amd64/ARM 고정쌍·local S3/versioned rollback·기존 PG image 흐름 범위       | 현재 final SHA/ARM S3·Mac Docker Desktop/rootless/Podman 범위 구분; 과거 개발 upgrade는 현재 gate에서 제외        |

외부 witness 잔여를 수락된 Rust 연산의 미구현으로 되돌리지 않는다. 기존 비차단 platform 한계를 새 필수 선행으로 만들지 않고 기존 F도 근거 없이 제외하지 않는다.

### 5.3 로드맵

**0.6.0 필수**는 위 세 backend·ON/OFF 사용자 흐름·현재 신규 설치12단계·현재 데이터/반출복원·동등 작업량 비용·최종 검토/CI 수락이다.
**0.6.x**에 Storybook/Playground·CJK/frontend 정리를 둔다. **공개 베타는0.7.0**이며0.6 완료의 새 필수 조건이 아니다.
#280 협업 엔진 비용·복구/schema 비교와 승인된 썸네일 검토는 기존 선행 뒤 별도 scope로 진행한다. 기본 엔진 채택·사용자 데이터 이전은 비교 뒤 별도 결정이다.
TS 데이터 이전은 기존 사용자 확인으로 현재 범위 밖이며 HWP 썸네일·MCP HTTP·requeue 운영 API/UI·license issuer trust 등 원본 미제공을 자동 신규 구현으로 만들지 않는다.
#266 fixture/probe·#263 docs, recovered digest/import/search/streams/collab/deps WIP와 dependency 후보는 현재 실제 상태/고유 diff를 확인해 별도 소유권으로 이어간다. 완료됐던 작업을 자동 재가동하지 않는다.
mail 영속화/digest·import admission·DB/room/search/outbox/attachment 비용·DTO/권한 join·Prometheus·restore.env quoting·flushDelay/구독 분리 등 후속은 기존 근거와 연결하며 이번 문서로 구현/검사 완료를 선언하지 않는다.
