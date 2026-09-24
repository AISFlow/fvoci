# FVOCI Rust 재작성

## 기준선 (2026-09-24)

- 대상 AISFlow/fvoci: 초기 `97a3fe61ede69390b78beaf2de8dd394ad49eed1`, 통합 브랜치 `fvoci/daggertooth`. 기존 미추적 starter 파일을 검토·보존하며 시작.
- 원본 fvoci/FVOCI main: `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`.
- 열린 PR #999: base `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`, head `393795261322b916e588043cf94feca999175843`, 미병합. 기능 조사 기준은 이 HEAD; main과 차이는 별도 검토.
- 원본은 비공개 별도 참조 clone. 공개 문서 서비스에 소스 전송 금지. 기존 원본 개발 checkout은 수정하지 않음.

## 현재 수락 지점

전체 재작성은 **부분 구현**이다. Run `run_b01d432a9dee`.

- PR1 인증 slice merged `fe30bd1`; 원격/실제 DB·HTTP 검증 완료.
- [PR2](https://github.com/AISFlow/fvoci/pull/2) native 추출 merged `1fc8af347246b1ae26881103d621268155c5c3d7`.
  검증 HEAD `0d3fb355119adeea8309c3d9a53ba3804dab6cb1`, 최신 로컬 fmt/clippy와
  production/test-hang 각52개 성공(ignored0). 원격 Rust35908128293 fast/postgres,
  Native35908128283 actual52+52 성공. Fable12140a6+0d3fb35 차단 없음.
  기대 HEAD를 지정해 squash merge하고 actual merged/main 반영 확인. Post-merge CI 대기.
- Workspace backend `bf1ab03`: 로컬 lib7/DB66와 Fable 검토 완료, 원격 수락 전.
  통합은 `rust-workspace-integration`에서 새 main을 병합했다. UI는 Composer
  `task_ebca570d74e9 / ctx_4761880b5c3a` (`rust-workspace-web`) 구현 중.
- 다음 문서 기반은 Grok `task_76f95af84a63 / ctx_db38f9d1f96e`
  (`rust-wiki-document`, base d5da209): 신규documents route/DB/004/grants와 독립검사.
  UI와 파일 소유권을 분리했고 production router 연결은 UI 제출 뒤 코디네이터 담당.
  Workspace/UI PR 수락 뒤 문서/협업 후속 PR로 통합하며 stacked PR은 만들지 않는다.

| 기능 | 원본 근거 | 보존할 외부 동작·불변식 | 새 구현 | 검증 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·세션·프로필 | identity/routes.ts, core/auth.ts, pg/identity-access.ts | 활성 사용자·철회·프로필/이벤트/감사 원자성 | 검증 완료, PR1 merged | 실제 PostgreSQL/HTTP 및 CI | 확장 인증·정책·UI 연결 |
| 첫 workspace | domains/workspaces, contracts/workspaces | 현재 역할·철회·원자성·RLS | 첫 backend/React 수락5d8cac8, PR4 merged | lib12/DB66 양 아키텍처/React6/CI/Fable | counts·quota·groups·members-list 등 미구현 |
| HWP5/HWPX 본문 | 원본 추출 경로, pinned rhwp e8800c8 | 실제 본문·빈/부분/손상·자원 한도 | native component 검증 완료, PR2 merged | 로컬·CI52+52/Fable | 첨부 권한/업로드/저장/검색/썸네일 미연결 |
| 문서·협업 | domains/documents/collab, 기존 React/Tiptap | 문서 권한·provider envelope·철회·CRDT 저장/복원 | wiki PR5 수락; codec/DB/native engine PR6 수락, /collab 미연결 | PR5/6 실제 앱역할 DB·양 아키텍처 CI/Fable 통과 | 실제2UI 편집·awareness·재접속·persist·새 프로세스 복원 후 편집 |
| 나머지 제품 | 아래 범위 보존 목록 | 원본 기능·보안·데이터 계약 | 재작성 미착수 | 미실행 | 프로젝트/태스크/첨부/검색/알림/운영 등 |

## 설계·자원 결정

작은 단일 Rust 서버 + PostgreSQL로 시작한다. worktree별 target, 실행별 DB/역할/스토리지, port 0 사용. 빠른 순수 정책 검사와 실제 DB 검사를 분리한다. 동시 쓰기 워커 최대 2개, 무거운 검증 한 묶음. 원본 다중 DB 미완료 범위와 재작성 미구현을 구분하며 첫 단계 성공을 전체 완료로 표현하지 않는다.

## 재개

AGENTS.md → .agents/environment.md → Orca Run task-list → 이 문서 → git status/worktree와 실제 프로세스를 대조한다. 진행 중인 작업을 중복 배정하지 않는다.

## 초기 착수 기록 (현재 수락과 구분)

공통 기반 d692134에서 Composer task_3377f29385d5 / ctx_6f56b812c5f0이 독립 rust-profile-slice worktree에 첫 기능을 구현 중이다. 원본 프로필은 PATCH /api/v1/auth/me, 엄격한 givenName/familyName/locale/timezone/weekStartsOn/textScale 입력, fvoci_session 쿠키, sessionUserOutput 반환. 정지 경합은 401 authentication_required이며 본문과 이벤트가 남지 않아야 한다. 원본 이벤트는 user.name_updated (감사 비활성); 이번 요구에 따라 감사도 함께 원자 기록하는 의도적 차이를 둔다. 프론트 계약 정본은 Rust DTO로 두고 클라이언트 생성 여부는 통합 시 확인한다.

## 범위 보존 목록

아래 원본 근거는 모두 고정 PR HEAD 기준이다. 목록은 지원 선언이 아니다.

| 기능 | 원본 근거 | 보존할 외부 동작·불변식 | 새 구현 | 검증 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 인증 확장·PAT·OIDC·MFA·사용자 생명주기 | packages/contracts/src/routes.ts auth/me/admin, core/auth.ts | 세션 폐기, 범위, 마지막 관리자, 탈퇴/복구 | 재작성 미착수 | 미실행 | 첫 로그인 외 전체 |
| 워크스페이스·멤버십·그룹·인가 | routes.ts workspaces/groups/apiTokens, server domains/workspaces | 현재 권한, 철회 경합, 테넌트 RLS·풀 컨텍스트 | 첫 backend/React 수락5d8cac8, PR4 merged | 실제 앱 역할DB66 양 아키텍처/React6/CI/Fable | groups/확장 정책 등 미구현 |
| 프로젝트·태스크·일정 | server domains/projects/tasks, routes.ts ics/holidays | API·공유/멤버 권한·일정 의미 | 재작성 미착수 | 미실행 | 전체 |
| 문서·위키·댓글·공유·리비전 | server domains/documents/comments/share | 저장 형식·리비전·읽기/쓰기 권한 | wiki 생성/조회/metadata 부분 구현 PR5 | 워커DB13, 통합/UI 검증 진행 | 본문 편집·댓글·공유·리비전 미구현 |
| 협업 | server 협업 구현, editor/package.json | Hocuspocus 4.6.0, Yjs13.6.32, Tiptap3.31.3, 두 클라이언트·철회·재시작 | codec/DB/native engine 기반 PR6 수락, 제품 미연결 | provider fixture11/native35·38 양 아키텍처 검증 | 문서/인가 기반 뒤 durable Yrs·실제2UI 수락 필요 |
| 첨부·local/S3·추출·썸네일 | server domains/attachments, packages/storage | 다운로드 인가·지원 형식·취소/자원 제한 | 재작성 미착수 | 조사 중 | native 대체 실증 필요 |
| 검색·색인·AI | server domains/search, packages/search, routes.ts ai | 검색에서도 인가·철회·색인 복구 | 재작성 미착수 | 미실행 | 전체 |
| 알림·메일·webhook·연동 | server domains/notifications, packages/jobs, routes.ts github/webhooks | outbox·커밋 후 전달·중복/재시도 | 재작성 미착수 | 미실행 | 전체 |
| 동의·감사·사용권 | routes.ts legal/auth.consents/admin.audit, packages/ee | 동의 gate·증거·서명·권한 | 재작성 미착수 | 미실행 | 프로필 감사 강화 외 전체 |
| MCP·CLI·설치·백업·복구 | server src/init.ts/backup.ts/doctor.ts, 제품 MCP | 프로토콜·오류·복원·운영 취소 | 재작성 미착수 | 미실행 | 첫 설치 외 전체 |
| SQLite/libSQL/Turso 및 이관 | PR999 packages/db | 원본 제공 범위와 목표 구분 | 재작성 미착수 | 미실행 | PG 우선; 원본 전체 다중 DB 완료로 간주하지 않음 |
| 프론트엔드·배포·ARM64 | apps/web, packages/editor, 배포 리소스 | 한국어·접근성·편집 흐름·정적 자산 | 재작성 미착수 | 미실행 | 최초 실제 HTTP slice 이후 연결; 이관/운영/ARM64 지원 미선언 |

## PR 및 원본 회귀 확인

- 사용자가 대상 PR #1 (`fvoci/daggertooth` → main)을 직접 열었다. 같은 범위의 중복 PR은 만들지 않는다. 2026-09-24 추가 승인 이후 독립 기능의 후속 PR 생성과 수락 후 머지는 허용된다.
- PR999 현재 CI35877360079: PG16/17/18 database-contract, image-tests, native standalone-arm64 성공; test의 `bun run test:pkg --affected`와 집계 report 실패. 원본 문서의 이전 성공 수치를 이 CI의 성공으로 대체하지 않는다.
- 원본 확인: 설치 GET/POST /api/v1/setup, 성공201 {userId,workspaceId}, 재설치404 instance_setup_already_completed. 로그인 POST /api/v1/auth/login, 성공200 {userId}, 누락/오류/정지401 invalid_email_or_password. 설치는 실제 첫 workspace/owner도 원자 생성해야 한다.
- 협업 위험 probe: Grok task_1532bb645f74 / ctx_00b1baf38504, rust-compat-probe worktree, base31b6790, 단독 소유 compat/**. 저장 updateV1과 Hocuspocus 프레임 호환을 따로 확인하며 실제 UI/권한 검증 미실행을 감추지 않는다.
- 의존성 공식 확인: docs.rs axum0.8.9/SQLx 및 crates.io 버전 메타데이터(axum0.8.9 MIT/Rust1.80, SQLx0.8.6 MIT OR Apache-2.0, Tokio1.47.1 MIT/Rust1.70, Serde1.0.228 MIT OR Apache-2.0). 선택 실제 버전은 Cargo.lock에서 고정·검증한다. 이 조회는 벤치마크가 아니다.

원본 CI 실패 로그 확인: apps/server/test/init.test.ts:119의 `usage 문자열에 init` 정규식 검사가 중첩 CLI usage의 `>`에서 실패했다. Rust에서는 이 소스 텍스트 정규식 하네스를 복제하지 않고 실제 CLI 호출을 검사한다. 이 원본 실패를 수정하거나 원본 PR에 쓰지는 않았다.

## 협업·문서 초기 위험 검증 — 통합 d154a60

Grok 제출 e94ac2d를 통합한 d154a60에서 코디네이터가 다시 실행했다. `npm --prefix compat/js ci --ignore-scripts --no-audit --no-fund` 성공(51 packages, 0.9s), `cargo build --locked --offline --manifest-path compat/Cargo.toml --bins` 성공(별도 비어 있던 target, 캐시된 crate, 6.25s). `YRS_BRIDGE=.../compat/target/debug/yrs-bridge node compat/js/probe.mjs` 6개 성공(0.141s), `node compat/js/hocuspocus-handshake.mjs` 실행 성공(0.472s) 및 프레임 불일치 재현. `compat/target/debug/extract-probe compat/fixtures/sample.{pdf,docx,hwpx,hwp}` 실행 성공(0.002s): 생성한 유효 형식 fixture의 PDF literal/DOCX·HWPX XML 토큰만 확인, HWP 본문 parser 없음.

이 결과는 제품 협업/추출 지원 수락이 아니다. Yrs0.23.5와 Yjs13.6.32의 gc:false updateV1 왕복·상태벡터·후속 편집은 확인했지만 전체 Tiptap 확장, 실제 FVOCI 두 UI, awareness 동작, 인가/철회, WebSocket 종료·재시작은 미실행. Hocuspocus4.6.0의 document-name/type/Auth/Stateless 프레임 어댑터와 HWP 본문/운영 수준 추출·썸네일 구현이 남았다. 제품 런타임에서 compat JS를 호출하지 않는다. 상세 범위·fixture 출처는 compat/README.md와 fixtures/NOTICE.md.

통합 후 probe runner는 호스트 전용 기본 경로를 제거하고 locked/offline 빌드 및 Node 검사별 30초 상한으로 정리했다. `bash compat/run.sh` 재실행 exit0, warm0.704s. 라이브러리 코드는 변경하지 않았다.

## 중간 독립 검사 (775f64d, 아래 최종 검사로 대체)

네이티브 x86_64, Rust1.98.1, PG18.3; 공유 crate 다운로드 캐시만 재사용하고 통합 target은 새로 빌드했다. `cargo fmt --check` 성공, `cargo check --locked --offline --all-targets --features db-tests` 성공(9.99s). `cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings`는 6개 진단으로 실패. 별도로 `cargo test --locked --offline --lib`: 4개 성공, build10.84s/본문5.62s/전체16.53s. `TEST_DATABASE_URL=<private-file> cargo test --locked --offline --features db-tests --test db_integration`: 13개 성공, build3.05s/본문8.48s/전체11.60s. 이 결과는 원본과 동등 범위의 성능 비교가 아니다.

프로필 감사는 원본 audit:false와 달리 사용자 요구에 맞춰 함께 커밋한다. 첫 버전에서 발견한 세션 철회 중 쓰기, null/누락 구분, 신뢰되지 않은 forwarded IP, 테스트 자원 공유 경로 및 병렬 migration 문제는 후속 Composer task에서 수정 중이다. Fable은 고정 제출 SHA6a77f76을 독립 검토 중이다. CI workflow를 추가했으나 원격 실행은 아직 하지 않았다.

### 독립 검토 결론

Claude Code Fable5.1 medium(task_43a2bfe9a062 / ctx_f447be92fcf1)의 SHA6a77f76 검토 완료: `familyName:null` 보존 버그와 세션 철회 후 프로필 쓰기는 수락 차단. 병렬 migration, 신뢰되지 않은 forwarded IP, per-email 잠금 범위, SIGTERM, 한글 길이/429/JSON 오류 계약, Argon2 취소 시 permit 소유권도 보강 대상으로 확인했다. 실제 검토 완료는 제품 수락을 의미하지 않는다. 검토 terminal은 release했고 후속 Composer task_12716d8c1cc0 / ctx_cf69c321c975가 수정 중이다. Grok task_6678e958475f / ctx_d8b2fe334de7는 고정 SHA775f64d의 비밀번호·HTTP 계약만 읽기 전용 대조 중이다.

위 내용은 당시 보류 기록이다. 후속 완료·검증과 다음 시작점은 아래 최종 수락 기록을 따른다.

## 최종 수락·검증 (195a58f)

Composer 후속 e2a7380/31dda85/a240805를 순차 통합했다. Fable `task_4aced35f7177 / ctx_ee0d5cd5054f`는 Claude Code `claude-fable-5-1`, medium으로 고정 a240805 + 공유 자원 수정25235f3을 읽기 전용 검토했고 첫 프로필 slice의 차단 결함 해소를 확인했다. 코디네이터는 이후 195a58f에서 malformed JSON의 problem 응답과 실제 IP 제한 회귀 검사를 보완했다. 해당 마지막 작은 diff는 코디네이터 검토·실행 증거이며 Fable이 그 SHA를 검토했다고 표시하지 않는다. Grok `task_6678e958475f`는 고정775f64d의 비밀번호/토큰·주요 HTTP 응답을 원본과 대조했고, 429 계약 차이는 수정했다.

환경: 네이티브 Linux x86_64, Rust1.98.1, PostgreSQL18.3. 최종 검사는 캐시가 있는 통합 worktree 전용 target에서 수행했다. 모든 아래 성공 명령은 exit0이다.

| 명령 / 검증 | 결과 | 시간 |
| --- | --- | --- |
| `cargo fmt --check` | 성공 | 0.113s |
| `cargo check --locked --offline --all-targets --features db-tests` | 성공 | 0.702s |
| `cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings` | 성공 | 0.846s |
| `cargo test --locked --offline --lib` | 6개 성공 | build0.76s / 본문5.64s / 전체6.467s |
| `cargo test --locked --offline --features db-tests --test db_integration` (`TEST_DATABASE_URL` 비공개 환경) | 24개 성공 | build1.95s / 본문26.41s / 전체28.416s |
| `cargo build --locked --offline --bins` | 서버·migration CLI 성공 | 1.753s |
| 실제 `fvoci-migrate`/`fvoci-server` + Python HTTP 클라이언트 | 두 UUID DB/앱 역할·동적 포트, 설치/로그인/프로필/null/인가 거부/로그아웃, SIGTERM exit0·재시작 후 세션/한글·이모지 프로필 보존 | 전체6.582s |

DB 검사는 원본 앱 역할 제한과 같은 종류의 실제 비특권 역할로 실행했다. 정지·세션 철회 후 쓰기 거부, 프로필 이벤트/감사 실패 rollback, 설치 감사 실패 rollback, 동시 첫 관리자 단일 승자, 누락/null/외부 사용자 ID, 비밀 컬럼·migration 메타데이터 접근 거부를 포함한다. 추가 동시 migration 검사는 두 연결이 advisory lock에서 대기함을 `pg_locks`로 관찰한 후 해제하여 최초 스키마가 한 번만 생성됨을 확인한다. 사용자·테넌트 컨텍스트 누출/RLS의 전체 수락은 아직 없는 첫 workspace API에서 수행해야 한다.

`bash scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test db_integration`을25235f3에서 실제 호출: 23개 성공, 환경/빌드/본문/정리 전체32.908s. 명령 실패(exit23)에서도 해당 컨테이너와 credential 파일이 정리됨을 별도 확인(2.643s). 최종195a58f에서 DB URL을 제거하고 `cargo test ... --test db_integration db_tests_require_database_url -- --exact` 호출은 의도대로 exit101/1개 실패/skip0이었다. 실제 HTTP 진단 스크립트는 이 실행의 로컬 임시 증거이며 제품/CI 의존성이 아니다.

중간 실패는 숨기지 않았다: 최초 clippy6건, helper의 없는 uuidgen(exit127) 및 exec에 의한 EXIT 정리 누락, 새 병렬 migration 테스트의 SQLx Send 컴파일 오류와 fixture의 public 함수 초기화 누락을 수정했다. 마지막 전체 gate에는 실패가 없다. timeout 증가·전체 재시도로 덮지 않았다.

피드백 예산은 현재 slice 기준 warm 빠른 gate10s, DB gate35s로 시작한다(최종 각각 약8.1s,28.4s). DB 후반의 로그인 제한 검사는 실제 Argon2 검증을 반복해 시간을 사용한다. 원본과 동일 범위·조건의 비교 벤치마크는 미실행이므로 속도 개선을 주장하지 않는다. CI YAML 파싱은 확인했으나 원격 Actions 실행·ARM64·이미지 배포·기존 데이터 이관/복구·브라우저 E2E는 미실행이다.

### 남은 차이와 다음 구현 지점

- 사용자 요구에 따라 프로필 감사 원자성을 추가했다(원본 audit:false). 세션 철회 중 쓰기 차단도 원본의 경합 공백을 강화했다.
- 초기 새 DB 전용이다. 기존 설치 업그레이드/이관은 지원하지 않는다. MFA/OIDC/PAT/동의 gate/설정 기반 비밀번호 정책·좌석 제한·계정 생명주기는 미구현이다.
- 제한기는 프로세스 로컬·직접 socket IP 기준이다. 신뢰 프록시·분산 제한은 미구현이며 프록시 뒤에서는 IP 버킷을 공유한다. 장기 프로세스의 한도 초과 시 제한기 키 교체 같은 잔여 정책은 후속 보강 대상이다.
- 이벤트는 DB에 원자 저장하지만 outbox 외부 전달/재시도 작업은 미구현이다. Rust DTO 기반 OpenAPI/TS 생성 및 기존 UI 연결도 남았다.
- 협업 adapter/awareness/권한 철회/재시작 복원, 실제 HWP 본문·운영 추출/썸네일은 초기 probe를 넘어 검증하지 않았다. 나머지 기능은 위 범위 보존 표를 따른다.

다음은 인증 기반 위 첫 workspace 연산: 현재 멤버십 인가, 실제 앱 역할 RLS·철회 경합·풀 컨텍스트 재사용 검증, 기존 React 호출 연결이다. 먼저 AGENTS → environment → 이 수락 SHA와 Orca task → git status/worktree를 확인한다. 재현 명령은 `cargo fetch --locked`, 빠른 gate, `scripts/start-test-postgres.sh`이며, 컨테이너/DB는 매 실행 새로 생성한다. 대상 PR #1은 사용자가 만들었고 이 작업에서는 push/PR 생성/원본 원격 쓰기를 하지 않았다.


## 2026-09-24 후속 기능 재개 (진행 중)

- PR #1 실제 재개 상태: Ready/open, HEAD `f5faf89fa38e00ae4772f61db912b826c8a68818`; 사용자 관찰의 Draft와 달랐다. 미커밋 변경 없음. 과거 push 미실행 기록 이후 사용자 요청으로 해당 HEAD가 push된 상태였다.
- CI 수정 코드 `864a41e4e4da9988390e695655946318dbf8c22d`: job context를 사용할 수 없는 postgres job env에서 DB 검사 step env로 동적 포트 URL 이동. 실패한 push 실행35894918085/checksuite97197656447은 check-run/job0개여서 API annotation 목록이 없었고, gh는 workflow issue로 표시했다. 일반 YAML 파싱을 Actions 검증으로 간주하지 않았다.
- 원격 [Actions35898070365](https://github.com/AISFlow/fvoci/actions/runs/35898070365), 동일 HEAD PR 이벤트: fast/postgres 모두 성공. 실제 lib6/DB24, ignored0. 원격 cold DB compile41.48s+본문55.21s, lib compile31.79s+본문8.67s. 고정 Actions 두 SHA 조회 성공, Rust1.98.1 공식 manifest200 및 CI설치 성공, PG고정 이미지 CI초기화·실행 성공. 로컬 warm과 다른 장비/조건이므로 단순 속도비교하지 않는다.
- 동일 SHA 로컬: fmt0.31s/check0.51s/clippy0.51s, lib6본문5.61s/전체5.88s, `scripts/start-test-postgres.sh` DB24본문27.63s/준비·정리포함30.38s, 모두0failed/0ignored. 기존 fast10s/DB35s 로컬 예산 이내. 테스트 소유 컨테이너 정리됨.
- PR1 제목/본문을 실제 인증 제품 코드·검증·미구현 범위로 수정했다. `gh pr edit`는 deprecated projectCards GraphQL 오류로 실패하여 승인된 GitHub REST PATCH로 갱신했다. rulesets없음/main보호API는 Branch not protected404; 이를 검사 면제로 사용하지 않는다.
- 독립 PR1 검토: Claude Code Fable5.1 medium task `task_f82e7dfa064b` / dispatch `ctx_655c9221016b`, 고정864a41e, `rust-pr1-final-review`; 아직 결과 대기. PR1 머지 미실행.
- Workspace: Composer2.5 task `task_d6e1a47da19a` / dispatch `ctx_8acea26b1479`, base864a41e, `rust-workspace-slice`; 소유 src/**, tests/**, 신규003_workspace.sql, grant-app-role.sql. 원본 계약·RLS·철회 경합 구현 중, 미수락. 공통 manifest/lock/CI는 코디네이터 소유.
- Native rhwp: Grok4.6 task `task_d8508aa82dc4` / dispatch `ctx_ecd7e7cf88f4`, base864a41e, `rust-rhwp-extract`; 소유 crates/document-extract/**와 독립 manifest/lock/fixtures. native검증/제품첨부연결 모두 아직 미수락. 무거운 검증 슬롯은 rhwp에 배정, workspace DB는 조정 후 실행.
- 두 Cursor 실행 receipt의 요청/유효 모델 일치 및 실제 TUI 작업/프로젝트스킬 읽기 확인. Fable 요청/유효 claude-fable-5-1 medium 일치, turnStart observed. 기존 Run 유지, retained/user_takeover와 소유 불명 fvoci-rust-test-pg-kinesis 및 다른 runner 컨테이너 보존.
- 다음: Orca `orchestration check`의 전체 delivery 처리 → 고정HEAD Fable지적 해결 → PR1 조건 충족 시 기대HEAD로 squash merge → 최신 main에서 후속통합 worktree/PR. Workspace backend 수락 후 기존 React/DTO생성 연결, rhwp native 수락 후 실제 부모권한·첨부 연결. 현재 Hocuspocus probe의 미지원 상태는 그대로다.

### PR1 독립 검토와 사용자 fixture 추가

- Fable `ctx_655c9221016b`는 864a41e 제품 전체 및 이후 `bea324300d133d0fa5b81880fef9ea50c132e451` fixture-only diff를 검토했다. 인증 slice의 차단 지적 없음. 보고서 `/tmp/fvoci-pr1-review-864a41e.md`, worker_done 수신 후 release 완료. source와 다른 새 workspace 제품 코드는 아직 검토·수락하지 않았다.
- bea3243은 사용자가 sample.hwp/sample.hwpx를 실제 Hancom 파일로 교체한 커밋이다. 사용자가 직접 파일을 확인하고 `안녕`을 입력했다고 확인하고 계속 진행하도록 승인했다. 기존 파일을 보존했으며 NOTICE/README를 실제 출처·기대 본문으로 수정했다. gen.py는 별도 빈 디렉터리만 받도록 변경하여 원본 fixture 덮어쓰기를 막았다. 임시 디렉터리 생성 성공, 비어 있지 않은/체크인 경로 거부, 기존 fixture SHA256 불변을 실제 검사했다.
- bea3243 원격 CI35898327395 성공, auth 제품 코드 변화 없음. 관련 로컬 `bash compat/run.sh` exit0/0.76s; 기존 Yrs6사례와 Hocuspocus envelope 차이를 유지했고 HWPX token은 `안녕`이다. HWP probe는 여전히 CFB/FileHeader만 확인하며 native 본문 추출 성공을 뜻하지 않는다.
- Fable 비차단 권고: 알 수 없는 세션 로그아웃의 불필요한 이벤트 저장은 Composer 소유 identity.rs에서 다음 slice에 회귀 검사와 함께 수정한다. 빈 familyName 정규화, timezone 제한, sliding cookie 갱신, hash 오류 진단, definer search_path 축소·반환 열 명시 및 관련 검사 보강은 추적 중이다. 기존 slice의 제한을 수락하는 것이며 전체 보안 동등성 선언이 아니다.

### PR #1 머지 완료와 후속 기준

- [PR #1](https://github.com/AISFlow/fvoci/pull/1) 실제 merged/closed 확인. 검증 HEAD `79d7b69a195edfa40b42f92e5b78797ec2e26bb2`를 기대 SHA로 지정한 squash merge SHA는 `fe30bd1b7c6f2632c354c4317c73969789de3f23`; 원격 main도 같은 SHA다.
- 최신 HEAD Actions35899398689 fast+postgres 성공(lib6/DB24, ignored0). 같은 HEAD의 로컬 fmt/diff/compat 및 generator 격리 검사 성공. Fable의 제품 보안 검토 SHA는864a41e, fixture delta는bea3243; 그 이후 제품 Rust/SQL/테스트/Cargo/CI diff는 없으며 문서·생성기 수정은 코디네이터가 검토·검사했다. 최신 전체 SHA를 Fable이 승인했다고 과장하지 않는다.
- post-merge push CI35900278533 진행 중. 다음 통합은 main fe30bd1에서 Orca가 만든 `/home/kinesis/orca/workspaces/fvoci/rust-workspace-integration`, branch `fvoci/rust-workspace-integration`에서 수행한다. 이전 daggertooth 및 작업 worktree는 미수락 코드 보존을 위해 삭제하지 않았다.
- Workspace 첫 제출 `eaec22c`는 빠른 검사만 완료되어 미수락. 후속 검증 task `task_ed526f3bccd5` / `ctx_0c2f0b3e544b`가 같은 Composer 터미널과 파일 소유권을 인계했다. Fable fixed-eaec22c review task `task_d8dd8e9f40cc` / `ctx_441329a7aad7` 진행 중. DB gate 및 보완 지적 해결 후 통합한다.
- Post-merge CI35900278533은 main fe30bd1에서 fast/postgres 모두 성공했다. Fable은 후속 메시지 msg_3230eda1ade0에서 79d7b69의 문서·생성기 diff도 별도로 검토해 수락 가능하다고 보고했다(전체 후속 workspace 승인 아님).
- Rust DTO 기반 계약 생성을 위한 공통 의존성은 선택 feature `api-schema`의 utoipa `=6.0.0`으로 고정한다. 공식 crates sparse index와 실제 .crate Cargo.toml에서 MSRV1.88, MIT OR Apache-2.0 및 공식 Git tag를 확인했다. 기존 Rust1.98.1과 호환 범위이며 기본 인증 검사에서는 feature를 활성화하지 않는다. lockfile은 utoipa6.0.0/utoipa-gen6.0.1을 추가하고 기존 패키지 버전을 바꾸지 않았다. 실제 DTO→OpenAPI→TS 생성 연결은 다음 UI task에서 구현하며 의존성 추가만으로 완료 표시하지 않는다.

### 후속 구현 검토·보완 체크포인트

- Workspace 후속 `41da1b58eeef8c3d316e905609acae964afe913a`는 워커가 DB41/41(42.84s)과 lib6/clippy 성공을 보고했으나 **미수락**이다. 고정 eaec22c의 Fable 보고서 `/tmp/fvoci-workspace-review-eaec22c.md`와 후속 diff 확인에서 감사 기록 원자성, 인가 전 개인 workspace 정보 노출, 삭제된 대상 사용자 변경, 개인 workspace FK/unique 보강이 남았다. 보고된 검사 개수만으로 기능을 수락하지 않는다.
- 보완 task `task_89edff0528f8` / `ctx_66cc86b4814d`는 같은 Composer 터미널·worktree와 src/tests/신규003/grant 스크립트 소유권을 인계했다. 이전 두 제출 커밋은 보존하며 통합 전 차단 지적을 수정한다. 다음은 고정 제출 SHA 검토와 통합 SHA의 실제 DB 검사다.
- rhwp task `task_d8508aa82dc4` / `ctx_ecd7e7cf88f4`는 native crate를 구현 중이며 아직 실제 native 검사·수락 전이다. upstream Git 전체 이력 fetch가 공유 crate 캐시를 막아 해당 소유 fetch만 중단했다. 정확한 rhwp revision의 얕은 sparse checkout을 준비 단계에서 검증하는 경로로 바꾼다. Cargo.lock만으로 path 의존성 내용이 고정되지는 않으므로 manifest metadata의 revision과 checkout origin/HEAD/clean 검사가 필수다. 테스트/build.rs에서 다운로드하지 않는다.
- 무거운 검사 슬롯은 Composer DB 종료 후 rhwp native build/test로 이관했다. 두 워커의 파일/target은 별도다. native 완료와 실제 부모 권한·첨부 저장·검색 연결 완료를 구분한다.
- 공통 통합 `43fc7bb`의 `cargo check --locked --offline --all-targets --features db-tests,api-schema` 성공(새 target17.71s). DTO/TS 생성 기능 자체는 아직 구현 전이다. PR #1 이후 추가 제품 코드는 아직 통합·push하지 않았고 후속 PR도 아직 생성하지 않았다.

- 후속 공통 `bb5c268`: 고정 tower-http0.6.11의 `fs` feature와 필요한4개 lock 패키지 추가. 실제 `cargo check --locked --offline --all-targets --features db-tests,api-schema` exit0/5.76s. Rust의 정적 자산 제공을 위한 준비이며 React 연결 완료는 아니다.
- Workspace `fcafb5d` 및 개인 workspace 원자 기록 delta `f209310` 제출. Fable `task_cfa3723946b8 / ctx_c0ca3ffbb59b`는 고정 fcafb5d와 별도 f209310 delta를 읽기 전용 검토 중이다. f209310 실제 DB57 중56성공/1실패/ignored0, exit101, 본문52.38s·전체55.27s. 실패는 fresh migration fixture가 스키마 삭제 후 해당 테이블 제약을 삭제하려 한 순서 오류이며 수정·재실행 중이다. 성공으로 기록하지 않는다. 기존35s 예산은 auth24개 기준이고 이번 검사 범위가 늘었으며 같은 범위의 속도 개선을 주장하지 않는다.
- Native 첫 제출 `222a80b`: Grok는25개 fixture/process 검사 및 clippy 성공을 보고했다. 코디네이터가 해당 task의 빌드된 native CLI를 사용자 HWP/HWPX에 직접 실행해 두 경우 `status:ok`, 본문 `안녕`, `used_preview_stream:false`를 확인했다. 단, helper 종료 검사가 실제 제품 경로를 충분히 검사하지 않고 출력 줄바꿈 한도·누락 내용 분류 결함이 있어 **미수락**이다. 보완 task `task_e512f2100714 / ctx_2e8a2222db50`에 동일 crate 소유권을 이관했다. 첨부·검색·썸네일은 미연결이다.


### Workspace integrated candidate bf1ab03 (2026-09-24)

- Composer backend submissions eaec22c/41da1b5/fcafb5d/f209310 and final regression
  handoff59a24d2 are preserved and integrated in `rust-workspace-integration`.
  Coordinator bf1ab03 replaces the partial fullwidth mapping with pinned
  unicode-normalization0.1.25 (MIT OR Apache-2.0, already transitive in the lock).
  Source3937952 `_shared.ts` requires NFKC then lowercase-only pattern: uppercase
  and surrounding whitespace are rejected, not silently lowercased/trimmed.
- Local exact bf1ab03: `cargo fmt --check`0.13s;
  `cargo check --locked --offline --all-targets --features db-tests,api-schema`0.17s;
  corresponding clippy `-- -D warnings`2.12s; `cargo test --locked --offline --lib`
  7 passed/0 ignored, body5.67s, total17.43s including fresh feature build.
  `bash scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test db_integration`
  66 passed/0 failed/0 ignored, build5.70s, body57.21s, total65.80s including
  ephemeral PG setup/cleanup. The prior35s budget measured auth24 tests; added
  workspace/RLS/revocation/rollback/upgrade cases change the scope. No speedup claimed.
- Actual app-role tests cover membership self-policy negatives, foreign tenant
  SQLSTATE42501, commit/rollback/error/dropped-transaction pool reuse, suspension
  and role/membership/session revocation races, event/audit rollback, concurrent
  owner demotion/removal exact404 loser and 001/002→003 migration plus grants.
- Fable fixed-fcafb5d/f209310 review found no remaining blocking product defect but
  required test corrections. Focused bf1ab03 delta review task6280ab04ca66 /
  ctx0446f2b9d341 is pending; this is tested candidate code, not final acceptance.
  Composer prior task89edff0528f8 is settled/released with clean worktree59a24d2.
- Current narrow API and deployment limits are in RUNNING.md. Counts remain zero
  placeholders, not computed aggregate parity; quotas/member-list/invitations/
  shared-view side effects/delete/UI remain incomplete. The Rust-only name and
  personal-workspace event+audit records intentionally strengthen atomicity.
- Native [PR2](https://github.com/AISFlow/fvoci/pull/2) is Draft at7034135.
  Rust35905000682 and documents35905000886 CI jobs succeeded, but independent
  Fable review found silent failed-section and table-caption omission. Grok task
  task_9b0113938837 / ctx_ac064a370261 owns crates/document-extract/** in its
  separate worktree and is fixing these blockers. Native acceptance and attachment
  product integration remain incomplete; no merge authorized by a green CI alone.
- Next: finish fixed-SHA delta reviews, accept backend only after closure, assign
  existing React flow and Rust DTO→OpenAPI→TS implementation to Composer; integrate
  native fixes into PR2, re-run relevant native gates and remote CI before merge.


### 사용자 추가 결정: 초기 협업 수락

서버 스택은 AGENTS.md의 고정 경계를 따른다. workspace 수락 뒤 기존 React
사용 흐름 → 최소 문서 권한/저장 → Hocuspocus4.6.0 envelope adapter + Yrs 제품
slice를 우선한다. adapter와 CRDT engine은 분리하고 document-name, Sync,
Awareness, Auth, QueryAwareness, Stateless persist/persisted/persist-failed,
Ping/Pong 및 close/error를 원본 provider에 맞춘다. 새 provider로 우회하지 않는다.

두 실제 React/Tiptap 클라이언트에서 동시입력·한글/이모지/문단, offline/reconnect,
중복/순서변경, awareness, 기존 연결 권한철회, persist/DB/socket 실패를 검사한다.
원본 clientId/state-vector/updateV1/gc 계약을 유지하며 새 프로세스에서 저장 CRDT를
복원하고 추가 편집 convergence까지 확인해야 수락한다. 현재는 모두 미구현이다.
문서별 task 소유권/idle eviction/flush/cancellation/join을 두고 종료는 새 연결 중단
→write 중단→flush→persist 확인→awareness/socket 종료→task join 순서다.
첨부 native 검증은 병행하되 무인가 업로드나 가짜 부모 리소스로 제품 연결을 대신하지 않는다.


### Workspace backend review closure and active UI task

- Fable5.1 medium `task_6280ab04ca66 / ctx_0446f2b9d341` reviewed fixed
  `bf1ab031933f5243cd91eceb87f703d9f9469ddc`, read the integrated logs and closed
  B1/B2/R1–R5. Report `/tmp/fvoci-workspace-review-bf1ab03.md`; no blocking issue
  for the narrow backend scope. Local backend is accepted; remote workspace CI,
  UI and whole-domain parity are still pending. Transaction-drop coverage is not
  an HTTP abort test. Default-feature unused import is assigned for correction.
- Composer2.5 `task_ebca570d74e9 / ctx_4761880b5c3a`, base `f1b97f0`, owns
  `rust-workspace-web` apps/web plus the explicitly scoped Rust schema/static
  transport changes and browser tests. It must reuse actual source React UI,
  generate TS from Rust DTOs, and run against real Rust+nonprivileged PostgreSQL.
  Root dependency/lock/CI/migration final ownership remains coordinator; only an
  exporter bin manifest entry was narrowly delegated. No UI result accepted yet.
- Grok4.6 `task_8f11568ea952 / ctx_c4d11404c451` is read-only for exact next
  document/collaboration contracts; no repeat of the already proven envelope
  mismatch. Source contracts will drive real initial document + two-client slice.
- Native fix4011be3 was pushed to PR2 after coordinator fmt/clippy and50 tests
  each production/test-hang passed, ignored0. Remote latest CI and Fable fixed
  delta review are pending; possible valid-empty HWPX classification remains under
  investigation through the pinned upstream public parser API. No merge yet.
## Native HWP/HWPX 추출 후보 — PR #1 이후

PR #1은 검증 HEAD79d7b69에서 squash merge되어 main `fe30bd1b7c6f2632c354c4317c73969789de3f23`에 반영됐고 post-merge CI35900278533도 성공했다. 이 후속 브랜치는 해당 main에서 분기했으며 workspace 제품 변경과 독립적이다.

- 구현: `crates/document-extract`, native edwardkim/rhwp revision `e8800c8def63449808a4092798442652ed460552`. 공개 crate 미게시를 확인하여 Cargo metadata의 불변 revision + 준비 스크립트의 origin/HEAD/clean 확인으로 path 의존성을 고정한다. lockfile만으로 path 내용이 고정된다고 주장하지 않는다. 라이선스/fixture 출처는 crate NOTICE에 보존했다.
- 본문: HWP5 압축/비압축 BodyText와 HWPX section/table 순서, 한글/이모지/빈 문서, 잘린 컨테이너·암호화/배포 문서·형식 불일치·제한 오류를 구분한다. 출력/깊이/지원하지 않는 본문 요소로 내용이 빠지면 Partial이다. 사용자 작성 Hancom HWP/HWPX 모두 실제 CLI에서 `안녕`이 나오며 PrvText를 본문으로 승격하지 않는다.
- 실행 경계: Linux native child, 검증한 입력/ZIP/출력 한도, exec 전 OS 주소 공간/CPU 제한, wall-clock/RSS 감시, timeout kill+wait 및 helper thread join. 동기 API이며 비동기 HTTP 스레드에서 직접 호출할 수 없다. 외부 요청 취소가 자동으로 child를 중단한다고 주장하지 않는다. Linux 이외 실행/ARM64는 미검증이다.
- worker: Grok4.6 `task_d8508aa82dc4 / ctx_ecd7e7cf88f4`의222a80b, 보완 `task_e512f2100714 / ctx_2e8a2222db50`의b0c4c82/625d940을 순차 통합했다. 완료 메시지를 확인하고 마지막 terminal release; worktree/커밋은 보존했다.
- 코디네이터 검증 SHA `7b05c269d06d57f523dc10c8cf5077596d53479b`: `cargo fmt --check`, `cargo test --locked --offline --all-targets --features test-hang -j 2` (32/32, ignored0, 전체4.11s), 같은 default-feature 검사(32/32, ignored0, 3.91s), `cargo clippy --locked --offline --all-targets --features test-hang -j 2 -- -D warnings` (exit0, 새 check 산출물54.54s). 모든 명령 cwd는 이 worktree의 crates/document-extract다. 이전 후보 c656ff3에서 독립 fresh 준비23.95s, 최초 test --no-run 빌드113.98s/최대RSS3126296KB를 별도 측정했다. 빌드 성공을 테스트 성공으로 바꾸어 기록하지 않는다.
- 자원: 32CPU/load3.34/가용42GB를 관찰한 뒤 이 별도 target의 초기 빌드만 jobs2로 제한해 별도 UUID DB 검증과 병행했다. 기본 Rust 인증 build에는 rhwp를 연결하지 않았다. 별도 Native documents CI는 explicit 준비/fetch 후 offline 검증한다.
- 현재 **미수락**: 독립 Fable 검토와 최신 원격 CI가 아직 남았다. 첨부 부모 권한·업로드·보존·추출 상태 저장·인가 다운로드와 검색 인덱스, 뷰어/썸네일은 미구현이다. native 추출과 제품 첨부 연결을 혼동하지 않는다. workspace/React는 다른 작업에서 진행 중이며 이 PR에서 완료하지 않는다.


### Native PR2 final correction candidate12140a6

- Grok fd6afc0 integrated as4011be3: failed-section handling, caption/form/equation
  extraction, bounded warnings/stdin and helper error classification. Local50 tests
  per production/test-hang passed; remote Rust35907475049 and documents35907475180
  execute at4011be3 (documents success confirmed). Fable fixed4011be3 closed earlier
  blockers and recommended public-parser empty-section refinement.
- Coordinator12140a6 uses pinned public HwpxReader/parse_content_hpf/
  parse_hwpx_section only for ambiguous HWPX empty sections, preserving package
  order. Valid zero-paragraph Empty, mixed-empty+body Ok, all-failed Corrupt and
  partial failure Partial are distinguished without custom parser or stderr inference.
  Allocator failure scans full bounded stderr, with a regression after800chars.
- Exact12140a6 stamped `/tmp/fvoci-native-final-gates.log`: fmt0.11s,
  clippy all-targets/test-hang0.62s, test-hang52 passed/0ignored total1.22s,
  production52 passed/0ignored total3.19s. Commands use `--locked --offline`.
  Tests comprise7 unit+37extract+8process-boundary; zero-test binary harness is
  not counted as test evidence. Actual fixtures include user-authored 안녕 plus
  independent multi-section/table/Korean/emoji/empty/malformed/resource cases.
- Fable delta task_6f8dcd8866c2 / ctx_faf9d9844108 reviewing12140a6. PR2 stays
  Draft until this closure and latest remote CI. Native component only: parent
  document permission/upload/storage/extraction-status/download/search/thumbnail
  product flow is still absent. No claim of attachment integration or full rewrite.
- Nonblocking diagnostics: repeated failed-section warnings count occurrences but
  retain only the first index; the warning-kind cap fallback is unreachable with
  the current7 kinds and remains a future diagnostic refinement. Neither hides
  Partial status. No actual Hancom-saved completely empty fixture was provided;
  independently generated valid empty documents are tested without claiming that provenance.


### 다음 문서/협업 설계 결정

원본 계약 조사 `/tmp/fvoci-document-collab-contract.md`의 wiki-only parentId/null,
WIKI 번호·tree/get·prosemirror/updateV1/gc:false 계약을 사용한다. 사용자 요구가
원본보다 우선한다: 원본의 persist 권한 재검사 누락을 복제하지 않고 현재 write
권한을 트랜잭션에서 보장한다. Node provider 검사만으로2실제UI 수락을 대신하지 않는다.
현재 단일 프로세스 범위에서 문서별 task를 소유하게 하되 Redis/다중인스턴스 기능은
미지원으로 추적한다. 데이터 손실 방지를 위해 공유DB에서 중복 room 소유와 오래된
snapshot 덮어쓰기를 방지하는 경계까지 다음 보안 설계 검토에 포함한다.


### 동시편집 보완 수락 계약 (진행 작업 유지)

사용자 보완을 현재 wiki task `ctx_db38f9d1f96e`와 UI task `ctx_4761880b5c3a`에
전달했다. 원본3937952 및 설치 provider를 기준으로 다음을 제품 구현에서 검사한다.

| 경계 | 필수 보장·회귀 |
| --- | --- |
| 인증/room | fvoci_session + 실제 Origin 정책 + 존재/소속/현재 ACL; token은 String(clientID) awareness 선언일 뿐 인증 아님; 각 논리 room 독립 인가 |
| clientID/readonly | claim 변경 시 기존 Y.Doc/미전송 이력 보존; struct의 과거 clientID를 socket claim으로 제한 금지; Update·SyncStep2 등 모든 변경 거부, 정상 readonly sync 유지 |
| 철회 순서 | 실제 공유 DB 잠금/조건으로 update-first와 revoke-first를 barrier 검사; 거부 update가 peer나 후속 정상 저장에 섞이지 않음; 철회 후 새 sync/broadcast/awareness 제한, 이미 전달한 데이터 회수 주장 금지 |
| 수락/영속화 | 입력 검증→현재 인가→durable bounded batch→공유 상태 반영/broadcast/ack 순서 또는 동등 보장; Yrs rollback/Undo로 DB 실패를 취소한다고 가정 금지; txn/lock guard를 await 넘어 보유 금지; commit 불명은 작업 식별·영속 상태 확인 전 성공 ack 금지 |
| persist barrier | flush 후 persist:id/persisted:id/persist-failed:id 문자열 유지; 연결·room별 앞선 처리 prefix를 고정해 commit 후 같은id 응답; 뒤 편집으로 무한 대기 금지; 거부/실패 prefix를 성공으로 응답 금지 |
| 삭제/정본 | state-vector만으로 dirty 판정 금지; delete set/pending update와 updateV1 정본 보존, JSON 재생성 금지; 순수/전체 삭제 및 reconnect/restart 검사 |
| 소유권 | 문서별 task + 지원하는 모든 쓰기 경로, 동시 최초접속/eviction재접속/늦은저장 검사; 단일프로세스 제한·공유DB 중복실행/stale-writer 거부, 자체 lease 플랫폼 금지 |
| awareness/구조 | 검증 사용자 정보·허용필드·연결세대별 claim, 오래된close가 새presence 제거 금지; UTF16 offset/API 검증; 중간삽입/삭제/선택/서식 및 실제 Tiptap 표/link/mention/참조/고유ID 보존 |
| crash | persisted 확인→기존client재전송차단→종료/크래시→freshclient DB복원→구조/삭제/후속편집→기존client복귀; commit후ack전/ack후/삭제only/flush경합/오래된저장 회귀 |
| 자원/수락 | frame 외 연결·room·큐·송신buffer·pendingCRDT·누적메모리·느린peer·빈도·decode/apply/shutdown 한도; 조용한drop후ack 금지; 합성 한글/composition과 실제 OS IME 검증 구분 |

Fable의 우선 독립 검토는 철회/수락 순서, DB 실패 후 공유 상태 오염,
persist·삭제-only barrier, fresh-client crash 복원이다. 기반 PR은 제한을 명시해
수락할 수 있으나 미완성 협업을 기본 활성화하거나 전체 협업 완료로 선언하지 않는다.


### CI 병렬화 후보 (2026-09-24)

- main 1fc8af3 이후 별도 CI 변경. 기존 fast/postgres/native-extraction check 이름을
  유지하고 PostgreSQL 및 native 추출을 ubuntu-24.04-arm에서도 실제 실행한다.
  총5개 독립 job, needs 없음, 각 Cargo jobs2. QEMU/larger/self-hosted runner 없음.
- 공개 저장소와 저장소 Actions enabled/allowed_actions=all 확인. 조직 전체 정책
  조회는403으로 제한되어 동시 실행 한도를 읽지 못했다. 실제 job 접수/큐 시간을
  확인하며 권한 확대나 결제 변경을 하지 않는다. 고정 PostgreSQL digest의
  linux/amd64 및 linux/arm64 manifest를 확인했다.
- workflow+event+PR 단위 concurrency로 같은 PR의 낡은 실행만 취소한다.
  push는 main만 유지하여 PR branch push 중복 실행을 만들지 않는다.
- Cargo 다운로드와 target 캐시 분리, OS/arch/toolchain/lock/manifest/feature/profile
  및 제품 소스별 key와 같은 의존성 prefix 복원. rhwp 공개 고정 source를 별도
  캐시하되 매번 origin/HEAD/clean 검증한다. 캐시 hit도 모든 테스트를 실행한다.
  CI debug info/incremental을 끄고 캐시 전송/빌드 비용을 측정한다.
- 이전 warm PR2: Rust35908128293 fast30s/postgres97s,
  Native35908128283 native77s(캐시복원32s). 당시 캐시6개 총4,180,767,358bytes.
  새 profile/architecture 최초 실행은 cold로 구분하며 warm 개선을 미리 주장하지 않는다.
- 로컬 Actions 표현식 검증: 공식 actionlint1.7.12 Linux AMD64 archive의
  SHA256 8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
  확인 후 실행. 일반 YAML 파싱과 구분한다. 원격 CI/독립 검토는 아직 미수락.
- 최신 사용자 지침에 따라 동일 통합 코드의 원격 수락 검사를 코디네이터가
  SHA/명령/실행 수와 함께 확인하면 같은 전체 검사를 로컬에서 반복하지 않는다.
  현재 제품 워커2개(UI/wiki)의 소유 경로와 로컬 heavy slot은 유지한다.


### CI PR3 수락·머지 및 현재 제품 작업

- [PR3](https://github.com/AISFlow/fvoci/pull/3) merged. 검증 HEAD
  f707a476af24dfdcb884958bea02a9b2e385c7e1, base1fc8af3, GitHub PR 검사
  synthetic merge ea994ef. 기대 HEAD를 지정한 squash merge 결과
  2fb61f9f7f51c6161e2b980ac05906c29a39891a가 실제 main임을 확인했다.
- Rust35910131684 / Native35910131855 cold attempt1 및 측정용 warm attempt2
  모두5job 성공. x64/ARM64 lib6·DB24, 각 native test-hang52+production52,
  ignored0. actionlint1.7.12 성공; Fable5.1 medium task1656eae5142d /
  ctx0bd3abce1e20 고정f707 검토 차단0, release. 주간 한도 미도달/Opus 미사용.
- job 초 단위 cold→warm: fast98→35, postgres111→90, ARMpostgres130→72,
  native178→28, ARMnative184→30. 이전 warm x64 fast30/postgres97/native77.
  한 번의 hosted sample이며 fast는5초 늘었다. 가장 느린 warm job97→90,
  native 캐시 복원32초→합계약5초. 테스트 범위는 ARM 추가 외 동일하다.
  새 native target cache236MB(x64)/227MB(ARM), 이전 main 통합cache1.36GB.
  캐시 누적은 관찰하되 타 작업 캐시 삭제나 quota 변경을 하지 않았다.
- main post-merge Rust35910871258 / Native35910871128 실행 중. CI만 수락이며
  전체앱 ARM 배포·첨부 연결·협업 완료가 아니다. 로컬 full gate 중복 실행 없음.
- 기존 workspace 통합에 최신 main을03a7509로 normal merge, append 문서
  충돌만 양쪽 보존하여 해결했다. Composer UI95da0dc를8f4c75a로 반영했으나
  오류 처리·정적 경로·stale 산출물·브라우저 권한 검사 보완 전 미수락이다.
  파일 소유권은 Composer UI, 코디네이터 CI/통합, wiki는 제출 후 반납 상태다.
- Grok wiki task76f95af84a63 / ctxdb38f9d1f96e, ca3193a(base d5da209) 제출/
  release. 실제 앱 역할 document_integration10/10, body8.35초/전체16.37초.
  Fable task4863a0e3f377 / ctxffd857259342 고정ca3193a 검토 중. 아직 제품
  router 미연결이고004 적용에 맞춘 기존 migration count 검사 갱신이 필요하다.
  workspace/UI 수락 후 최신 main에서 wiki 통합 및 협업 제품 경로로 이어간다.

- PR3 post-merge CI35910871258/35910871128도 실제5job 성공 확인.
- UI95da0dc의 null 누락·정적 fallback·오류 처리·mtime 기반 gate 및 권한별
  브라우저 검사 보완은 Composer `task_9799a8e15bca / ctx_54a9dbbcbc45`가
  rust-workspace-web에서 수행한다. 기존 terminal 재사용 readiness timeout
  `ctx_4d7ad5d92b50`는 실패로 기록했고, 완료된 이전 dispatch를 공식 release한 뒤
  같은 task를 지정 Composer2.5 새 terminal로 재시도하여 실제 실행을 확인했다.
- Fable wiki ca3193a 검토는 인가/원자성 차단0이나 icon:null clearing 결함,
  fractional 짧은 prefix panic과 실제 product revoke 경합 검사 보완을 발견했다.
  Grok `task_462531dfba7e / ctx_9a264a3d7793`가 같은8개 소유 경로에서 수정한다.
  UI E2E가 로컬 heavy slot 소유, wiki DB는 별도 배정 후 실행한다. 검토자는 release.

### Workspace/React 통합 수락 후보

Composer c8d438e를7905973에 통합했다. 해당 worker 기록은 lib11, static5,
실제 앱 역할/port0 Playwright4 성공(브라우저 약20.6초), clean/release다.
코디네이터는 누락/nullable 생성 계약을 추가 수정하여 응답 familyName과
emailVerifiedAt 필수키/null, PATCH 나머지 optional-but-not-null을 실제 parser와
대조하는 schema 검사를 추가했다. logout 네트워크 실패도 화면에 표시하고
세션 성공으로 오인하지 않는5번째 브라우저 검사를 추가했다.

통합 로컬 fmt/check/clippy(db-tests,api-schema), schema1, static5, TypeScript
검사를 수행한다. 새5번째 브라우저 및 기존66 DB 회귀는 독립 Web/Rust 원격 CI에서
이 통합 HEAD로 실행한 뒤 수락한다. 수정 후 예전 E2E4 성공을 새HEAD 성공으로
사용하지 않는다. 전체 원본 UI/인증·문서·협업 지원으로 확대하지 않는다.

### PR4 독립 검토 보강

7f30140 원격 fast/lib11, 실제 PostgreSQL66(x64/ARM64), static5/schema1,
React5, native52×2(x64/ARM64)가 모두 성공했다. Fable medium 고정 SHA 검토에서
설정 화면의 me 오류 분기가 hook보다 먼저 반환하는 B1을 발견하여 수락 보류했다.
후속 수정은 hook 순서를 고치고 실제 폐기 세션 쿠키로 설정 페이지를 다시 여는
브라우저 회귀를 추가한다. nullable workspace rename 스키마, 빈 목록 오류 문구,
index no-store, 실행 예시 bind/origin, 기본 feature 서버의 E2E 실행도 보강했다.
현재 변경의 원격 CI·추가 검토는 새 HEAD에서 별도로 확인한다.

비차단 후속: 개인 workspace 생성의 UI 연결, 알려지지 않은 오류 코드의 fallback,
전체 API path/runtime 자동 대조, dialog 포커스/접근성 복원, 설정 화면의 네트워크
오류와 인가 거부 구분. 이들을 완료했다고 표시하지 않는다. 문서 워커002bc57은
icon null/정렬 prefix/parentId 및 실제 멤버 변경 경합을 수정하고 DB13을 통과했으나
제품 router·migration4 통합은 PR4 수락 후 진행한다.


### PR4 수락·머지와 문서 통합 재개

PR4 https://github.com/AISFlow/fvoci/pull/4 는 수락 코드5d8cac8에서
Fable medium 추가 검토 차단0 및 원격 Rust35913683110/Web35913683261/
Native35913683122 전체6job 성공 후 squash merge했다. main 머지 SHA는
0617e7f9ed716a696eaa0ed80648959d09351477. lib12, 실제 DB66씩 x64/ARM64,
static5/schema1/React6(35초), native 양 아키텍처 성공. reviewer 종료 당시
진행 중이던 x64DB도 코디네이터가 66/0ignored를 확인했다. post-merge CI는 확인 중.

새 통합 worktree rust-document-integration은 main0617e7f에서 시작했다.
Grok 제출ca3193a+002bc57을 순서대로 통합하고 실제 제품 router에 wiki
생성/tree/조회/ancestors/body/metadata PATCH를 연결한다. migration004와
기존003 업그레이드·RLS·현재 역할/정지/폐기·atomic audit/event 검사가 범위다.
기존 승인 migration001–003은 수정하지 않는다. 원격 PostgreSQL job은 기존66과
새 document_integration13을 실제 실행하며, 통합/독립 검토 전 미수락이다.
문서 본문 변경·CRDT·첨부·프로젝트 문서·복구/이전은 아직 제품 구현이 아니다.

codec 제출f7a7307은 실제 provider4.6 고정 계약을 Grok task4b18c0b69cec/
ctx_a0b85bb5b3e4가 교차 검증 중이다. 아직 /collab 비활성. Fable 주간 한도는
도달하지 않았고 Opus로 전환하지 않았다. 기존 모든 작업 커밋/worktree 보존.

문서 통합 PR5: https://github.com/AISFlow/fvoci/pull/5 (Draft, 기준6e62e9f).
Composer task3aee2da6437a/ctx761ff4e13add, worktree rust-wiki-web가
apps/web, packages/i18n, src/api, HTTP documents DTO 및 generator의 단독 작성자다.
root 공통 파일·DB·migration·CI는 코디네이터 소유로 유지한다. Fable
 task18cb8e899c76/ctx7bf431f22c16는 고정6e62e9f DB/인가 수정 읽기 전용 검토.
Grok task4b18c0b69cec/ctxa0b85bb5b3e4는 rust-collab-wire의 codec/fixture만 수정한다.
동시 쓰기 워커2개, 로컬 무거운 검증 슬롯은 Composer의 실제 wiki E2E에 배정.
coordinator 통합 check/clippy(api-schema,db-tests) 및 lib19 성공; 원격 검사는 진행 중.

PR4 post-merge main0617e7f의 Rust35914216752/Web35914216744/Native35914216656 모두 success 확인.
문서 DB suite도 required-features=[db-tests]로 명시해 feature 없는 명시적
DB test 호출이 0개 성공으로 끝나지 않게 한다(다음 통합 커밋에 포함).

문서 UI 제출35e4538(8 browser 성공)을 bb77ec9로 통합했지만 새 헤더의 로그아웃
실패 처리·생성 오류 표시 등을 보강 중이다(Composer task9edd377f0505/
ctxf49beb3df3e1, UI 경로 단독 소유). 코디네이터는 문서 생성 DTO를 runtime과
OpenAPI의 같은 타입으로 합치고 parentId required-nullable/UUID, 응답의
present-nullable 필드, PATCH title/status의 null 거부를 원본 계약에 맞춰 고쳤다.
로컬 api-schema lib21/clippy/TS 성공, HTTP null 회귀는 다음 원격 DB 검사에서 실행한다.

codec45da379 교차 검증은 11개 성공(가짜4/6 opcode·routingKey·varint·trailing
입력 수정). 제품 미연결 상태를 유지한다. 협업 설계 자문에서 제안한 후보 Doc
방식의 decode 메모리 근거는 Yrs update.rs의 untrusted try_reserve 때문에
코디네이터가 재검토를 요청했다. 자문을 구현/검증 성공으로 기록하지 않는다.

문서 UI 보강5693f65를 f969a94로 통합했다. 워커의 실제 브라우저13/13(48.8초),
wiki-tree 단위1, build/typecheck 성공; 통합된 공유 DTO 변경은 원격에서 재검사한다.
통합 로컬 fmt/clippy와 api-schema lib21 성공. PR5 신규 UI/DTO 고정 SHA의
Fable 검토 및 원격 전체 gate를 통과하기 전 미수락이다. CI에 tree 단위 검사도 연결했다.
협업 자문은 decoder의 무제한 allocation/unchecked UTF8 문제를 확인했으나,
rlimit child만으로 UB의 보안 경계가 완성된다는 제안은 수락하지 않았다.
공식 Yrs 수정 버전/안전한 입력 경계를 확인한 뒤 제품 engine을 연결한다.

### PR5 수락 및 협업 구현 재개

PR5 https://github.com/AISFlow/fvoci/pull/5 는 d69fdd9951f1643ec8f63dcb330b26cdcb9fc7e4
에서 Fable5.1 medium 독립 검토 차단0 및 원격 전체6job 성공 후 머지했다.
main 머지 SHA421e70b19c5664b0468abc20b6b1616ed4ad4e3d, 상태 merged 확인.
검사 synthetic merge c70328a = HEADd69fdd9 + base0617e7f. Rust35917137028:
lib20, 실제 앱역할 DB66+13씩 x64/ARM64(ignored0). Web35917137015:
static5/schema1/tree1/실제 React13(1.1분). Native35917136993 양 아키텍처 성공.
Fable 읽기 전용 검토는 backend6e62e9f 및 최종delta d69fdd9의 실제diff를 확인했다.
비차단: 빈 제목 blur 복원, breadcrumb cache 갱신, tree 오류 직접 표시, 기존slug대소문자.
본문 편집/협업/첨부/검색은 아직 수락되지 않았다.

collab 통합7647250은 codec11검사를 통과한 c28b5e2에 최신 main421e70b를 merge했다.
codec 검사는 Hocuspocus framing만 보장하며 제품 /collab은 비활성이다.
임시 optional yrs0.23.5 의존성은 checkedUTF8 결함 때문에 제거한다. 원본 저장
형식을 변경하지 않고, 공식 yrs0.28.0 small-client/skip_gc/UTF16 후보의 실제
Yjs13.6.32 pending/delete/복원 호환성을 별도 native crate에서 검증한다.
이를 통과하기 전 엔진을 수락하거나 기존 설치 migration 지원을 선언하지 않는다.

Composer task2d8c8f185502/ctx809d0a92afb8: rust-collab-persistence, main421e70b 기준,
src/db/collab.rs·migration005·등록/앱권한·관련DB검사 단독 소유. 공통파일 중
위 migration/권한의 해당 범위는 명시적으로 위임했고 root manifest/CI는 코디네이터 소유다.
현재권한 재검사/철회와 append의 잠금순서, writer generation fence, op_id 조회,
원자성과 snapshot cutoff를 실제앱역할로 검증한다. 로컬 무거운 DB 슬롯1개 배정.
Grok 후속 native 엔진 작업은 crates/collab-engine만 단독 소유하며 root/API/DB를
수정하지 않는다. 두 쓰기 워커, worktree별 target/실행별DB자원 분리 유지.
다음 통합: 각각 제출SHA 검토 → 새 통합SHA 원격CI/독립검토 → 실제세션/Origin/
문서인가를 포함한 socket 및 기존 두 React/Tiptap 클라이언트 연결.
Fable 주간한도 도달 없음; Opus5.5 medium 대체는 아직 실행하지 않았다.

Grok native 엔진 task72f3f1594e9e/ctx2de4ab3cf2f0는 rust-collab-engine에서
crates/collab-engine(해당 manifest/lock 포함)의 단독 작성자다. 원본 Yjs13.6.32
fixture 생성과 네이티브 worker 경계 검증을 수행한다. 미수락이며 제품경로 비활성.

PR5 post-merge main421e70b의 Rust35917728923/Web35917728904/
Native35917728889 전체 success 확인. 다음 협업 기반 통합06e3242에서
fmt/clippy(all-targets,db-tests), codec11, actionlint 성공; DB/engine 제출은 진행 중.

PR6 https://github.com/AISFlow/fvoci/pull/6 Draft, 첫 HEAD34e1caa의
Rust35918208792/Web35918208561/Native35918208575 전체 success. 검사 merge ced4ae2
=34e1caa+main421e70b, codec11/lib20/DB66+13 양 아키텍처 및 기존React13 포함.
현재 DB/엔진 워커의 미제출 diff는 이 성공 근거에 포함하지 않는다.

원본 collab-http.ts의 STATE_OVERSIZE_FACTOR=8 및 config 본문 기본1048576을
확인해, 잠정1MiB CRDT 제한을 원본 기본8MiB로 정정했다. 초기 공통 한도는
update/snapshot8MiB, snapshot+tail 총32MiB/64행, native JSONframe48MiB다.
한도 초과는 명시적 실패이며 잘린 성공/부분 복원은 금지한다. 원본 환경변수
COLLAB_MAX_PAYLOAD_BYTES override 연결은 아직 미구현이다. 원본 저장 데이터의
이전 지원이나 대형 문서 전체 호환성을 이 기본값 확인만으로 수락하지 않는다.

### 협업 기반 제출 검증 (진행 중)

DB 제출3a08bc4를 통합0532bb0에 반영하고1ba6124에서 collab_integration을
실제 x64/ARM64 PostgreSQL CI에 연결했다. 로컬 all-targets Clippy 경고1건을
수정해 통과했고 actionlint 성공. PR6 HEAD1ba6124의 Rust35921661165,
Web35921661087, Native35921661026 전체6job 성공 확인. 알려진 receipt 결함과
독립 검토가 남아 해당 기능은 미수락이다.

DB 원본 제출 worker task2d8c8f185502/ctx809d0a92afb8는 유효 완료 메시지를
확인해 release했다. 앞뒤의 capability 누락/무효/폐기 완료 시도는 성공 증거에서 제외했다.
새 Composer taskc7428b1b7d31/ctx8ee8395734c7는 같은 persistence worktree의
receipt 권한·해시 기반 재확인·누적 조회 비용 보강을 소유한다. Fable medium
task736355da318b/ctxdafa8d332309는 고정3a08bc4를 읽기 전용 검토 중이다.
Grok engine9874a87 제출은 잠정2MiB 제한이 남아 미수락이며, 같은 터미널의
후속 taska755ad0b14fa/ctx445b177a1ece가 최종8/32MiB 계약과 복원 가능한
Apply 수락 경계를 보강한다. 통합 코디네이터는 native-engine CI를 준비 중이다.
Fable 실제 주간 한도 도달 시 Claude Code Opus5.5 medium으로 대체한다는
승인은 유지한다. 이번 검토의 요청/유효 모델은 Fable5.1 medium이며 대체 미실행.

재개: 위 dispatch 제출/검토 확인 → 고정 커밋별 통합 → 최종 원격 CI 및
독립 검토 → PR6 수락 판단. /collab 제품 경로·두 실제 편집기 수락은 아직 없다.

engine 후속3eb3ded를 a42d503으로 통합했다(기초9874a87→a80a4f6). 워커가
worker21검사4.96초/test-hang23검사6.04초를 보고했고, 통합 fmt 및 all-targets
clippy(test-hang)는7.60초 성공했다. Linux x64/ARM64 별도2job에 실제 native
검사와 production 테스트제어 거부·부모 no-default-features 컴파일을 연결한다.
Fable taskc7193b74e436/ctx05cc4dbef3ca는 고정3eb3ded 엔진을 읽기 전용 검토 중.
Grok 보강 dispatch445b177a1ece는 유효 제출 확인 후 release했고 worktree는 보존했다.

954421b의 native-engine CI35922263737은 x64/ARM64 모두 nested_any 회귀에서
stdout EOF/자식 종료 관찰 순서에 따른 Protocol 분류 실패로 중단됐다. 재실행으로
숨기지 않는다. Grok taska20ddcbc6736/ctx785570683988가 고정3eb3ded 이후
process 경계·회귀만 보강한다. Fable 엔진 검토는 고정3eb3ded에 계속 적용한다.

Fable DB 검토3a08bc4: receipt UPDATE/DELETE grant 결함 확인, CI 누락은
통합1ba6124에서 해소. hash+length+actor+seq만 남기는 append-only receipt와
임의100만회 lifetime hardstop 제거 설계를 자문 확인 후 결정했다. 오래된 receipt를
삭제하면 불명확한 commit 확인/op_id 중복 방지가 깨지므로 나이/cutoff 삭제는 금지한다.
작은 고정 크기 기록과 이벤트/감사 DB 저장량은 계속 증가하며 quota/자동보존기간
지원은 아직 주장하지 않는다. 복원 메모리·tail32MiB/64행 제한은 유지한다.
Fable DB dispatchdafa8d332309는 보고·추가 판단 후 release. Composer 보강은
c9124c9를 보존하면서 실제 동시 session revoke/크기경계/upgrade 앱권한 검사와
최종 receipt 설계를 진행 중이다. 검토 차단 해소·최종 CI 전 PR6 Draft 유지.

DB 보강c9124c9/e7c6751을5fdbe9f/c4e9f21으로 통합했다. 워커 보고:
collab22+순수collab4+기존DB66 성공(~77초), 자원 정리 후 release. 통합의
format 차이2곳을 정리한 뒤 all-targets clippy1.37초/lib24검사5.33초 성공.
Fable 고정e7c6751 delta task0e1aeca68e92/ctxbbe9ffa4b746 검토 중이다.

엔진 EOF 수정9a76f00은5e5bf2c로 통합했다. 워커는 CI와 같은 nodebug 조건의
process_boundary worker8/test-hang10 성공을 보고했다. Fable3eb3ded 검토는
EOF 분류 및 32MiB load 대비256MiB AS 부족을 차단으로 확인했다. 측정된
29.5MiB load의 VmPeak413MiB를 근거로 입력계약은 유지하고 AS1GiB/RSS512MiB,
최대8children의 최대 관찰 RSS예산4GiB로 조정·검증한다. 구조가 더 무거운 입력은
명시적 resource 실패가 가능하며 모든8MiB 문서 처리 성공을 보장하지 않는다.
요청별8초 wall deadline은 유지하고 누적CPU예산을 별도로 고친다. Apply의
복원 가능 크기 검사는 유지하되 매번 전체snapshot 전송은 제거한다.
Grok 후속 task89714a09237e/ctx987fc1e02c63가 같은engine worktree 단독소유.
Fable ctx05cc4dbef3ca는 보고 후 release; 후속delta는 다시 검토한다.

Fable DB 최종delta e7c6751: 차단0, receipt 권한/identity/성장 의미와 실제경합
보강 확인. dispatchbbe9ffa4b746는 보고 후 release. 남은 비차단 권고 중
세션 경합 overlap을 코디네이터가 추가 강화했다. 두 쿼리의 실제 lock wait를
pg_blocking_pids로 확인한다. 최초 좁은 blocker PID 가정은 wins 검사에서 실패;
PostgreSQL soft-blocker(앞서 대기 중인 append PID)도 포함해 수정했다.
동일 명령 `bash scripts/start-test-postgres.sh cargo test --locked --offline
--features db-tests --test collab_integration session_revoke_barrier` 재검사:
실제2개 성공2.48초(compile1.42초), 컨테이너 trap 정리, fmt/clippy0.62초 성공.
제품 DB 구현은 검토된e7c6751과 동일하다.

1899886 원격 native-engine35923500285 양 아키텍처 성공으로 EOF 수정 검증.
후속 메모리/누적CPU 한도 변경은 아직 미제출이며 이 성공에 포함하지 않는다.

엔진4b1039d를13812ed로 통합했다. AS1GiB/RSS512MiB, 요청별8초 유지,
누적CPU 예산 분리, Apply metadata 응답과 별도 Snapshot, Load 재사용 거부,
이미 전달된 frame/종료 순서 및 SIGABRT stack 분류를 보강했다. 워커 보고:
worker35검사10.05초/test-hang38검사12.34초, clippy각0.53/0.51초 성공.
작은96byte 텍스트 약65761개로7.36MiB snapshot을686ms에 생성하고 반복 tail과
함께 near32MiB 복원을 검사했다. 최초 fixture의 반복전체encode/뒤삽입은
준비 CPU 병목으로 중단했고, 앞삽입/최대4encode로 수정했다. 제품 timeout을
늘리거나 실패검사를 skip하지 않았다. 워커ctx987fc1e02c63 제출 후 release,
worktree/커밋 보존. 통합 fmt 성공. Fable5.1medium 요청/유효 설정의
읽기 전용 task38dbfeb88a25/ctx68339aebfecd가13812ed delta를 검토 중이다.
원격8job 최종 결과·독립 검토 전 PR6 Draft/미수락; /collab 제품 연결은 미구현.

### PR6 기반 수락 및 실제 동시편집 착수

PR6 https://github.com/AISFlow/fvoci/pull/6 merged. 수락 HEAD
`e0b6adecad2ebf59a17a5f9dd7d2a6eb80a7d69d`, 코드 검토13812ed,
GitHub 합성170374a(main421e70b). 머지/main SHA
`ba19932b03a98a69b24479e33ade68946b5bbd47` 실제 확인.
Rust35926096857/Web35926096961/Native documents35926096855/Native engine35926096916
8job 성공: lib24/codec11, DB22+66+13 각각 x64·ARM64 ignored0, React13,
engine worker35/test-hang38 각각 양 아키텍처. 최장 x64 DB job225초,
engine x64/ARM45/48초. 필요한 원격 검사와 동등한 전체 로컬 반복은 하지 않았다.
Fable5.1 medium 최종13812ed delta 검토 차단0; 독립 scratch native35/38,
실제 stack/순차 distinct-tail28.9MiB load 재현도 성공. ctx68339aebfecd release.
주간 한도 오류·Opus 대체는 발생하지 않았다.

남은 engine 비차단 회귀 권고: distinct-tail fixture 추가, 실제 stack 결과와
exit-first 경로의 결정적 검사. 중요한 제품 선행 위험: 서로 다른 client가 같은
위치에 대량 삽입한7MiB 상태가 Yrs/Yjs conflict scan 비용 때문에8초 reload를
넘겼다. helper는 kill/reap로 제한되지만 byte cap만으로 ack된 데이터의 복원을
보장할 수 없다. 실제 쓰기 수락 전에 proposed durable state의 bounded recovery를
검증하거나 동등한 보장을 마련하고 회귀로 고정한다. 제한을 늘려 숨기지 않는다.
전체snapshot capacity encode 비용(대표7.4MiB release100ms)도 batch 비용에 포함한다.
/콜랩 제품 경로·두 React/Tiptap·persist barrier·fresh-client crash는 아직 미수락.

다음 통합 worktree `rust-collab-live`, base ba19932. axum0.8.9의 공식 로컬
manifest ws→tokio-tungstenite0.29를 대조하고 ws feature와 native parent API를
연결할 의존성만 준비한다. root는 collab-engine default-features=false로
Yrs 엔진 빌드를 분리한다. 공개 registry fetch는 검사 전 명시 실행.
Grok task9d8a756baf43/ctx259ebcae441c: rust-collab-react base ba19932,
packages/editor/** 및 apps/web/**(generated API 제외), web manifest/lock 단독위임.
실제 source editor/session/schema 연결과 관련 검사를 구현 중. Cargo/CI/migration
및 통합 상태 문서는 코디네이터 소유다. 동시 쓰기 최대2 유지.
main post-merge Rust35927307111/Web35927307116 진행 중, native35927307145/35927307100 성공.
재개: 실제 task/미커밋 대조→Rust room/transport 작업 배정→고정 제출 통합→
실제 두 UI/인가·철회·삭제·fresh crash 검증 및 Fable 검토.

후속 공통 기준1ad5295: Composer task9c7b5948bb86/ctx62e83afccaab,
rust-collab-room에서 src/**, tests/**, RUNNING.md 단독소유로 실제 transport/actor/
현재 인가·durable recovery/persist 경계 구현 중. 원래005까지 migration과
rootmanifest/CI는 수정하지 않는다. Fable5.1medium taskb5d6a996852f/
ctx61175969f007가 새 recovery 검증/commit 조건의 좁은 읽기 전용 자문 중이다.
UI writer ctx259ebcae441c와 함께 쓰기2개, 서버 DB/native 무거운 검사는 Composer
하나에 배정했다. main ba19932 post-merge8job 모두 성공 확인.

코디네이터는 E2E 준비 단계의 native helper fetch, production worker 빌드 및
FVOCI_COLLAB_ENGINE 경로를 추가하고 worktree 안 독립 target/cache로 분리했다.
상대 CARGO_TARGET_DIR은 cwd 변경 전에 절대경로로 고정한다. bash -n과
기존 actionlint1.7.12 검사 exit0; 제품 UI/WS 통합 검사는 아직 미실행이다.
이 공통 변경은 새 기능 수락을 의미하지 않는다. scripts/CI는 계속 코디네이터 소유.

Fable taskb5d6a996852f/ctx61175969f007의 좁은 설계 자문 완료·release
(실제 Fable5.1medium, quota/Opus 전환 없음). 이는 제품 diff 수락이 아니다.
코디네이터 결정: 정확한 proposed snapshot+ordered tail+candidate를 새 helper의
단일 Load로 커밋 전에 검증하고 validation4초/recovery8초로 여유를 둔다.
미래 임의 부하에서도 시간 보장을 증명한다는 뜻은 아니다. Load만 성공한
8MiB 초과 복합 상태를 저장하지 않도록 validator Snapshot 출력 한도도 검사한다.
AppendCollabInput expected_tail_seq와 기존 호출부/회귀 변경을 Composer에 명시
위임했다. DuplicateAck 확인 뒤 실제 tx에서 예상 순번을 확인한다. 단일 actor가
private candidate를 쓰는 동안 join/sync/compaction을 공개하지 않고 실패 시
committed 상태로 재생성하는 경계는 유지 가능하다. DB 이전 전파는 금지한다.
room guard는 전용 detached connection/try-lock/명시 close, 최대4room·8helper.
철회 signal/poll만으로 새 전달을 인가하지 않고 sync/broadcast/awareness마다
현재 권한을 확인한다. Origin 부재는 원본의 비브라우저 cookie 정책을 유지하며
present malformed/multiple 값은 거부한다. 초기 WIP의 동기 recv와 root worker
feature는 작업자에게 수정 지시했고 rootmanifest/lock 원복을 실제 확인했다.

Composer 첫 제품 WIP47df897을25db2ba로 통합했으나 수락하지 않았다. 보고된
실제 PG/helper4검사와 wire11검사는 종료·철회·persist 보장을 충분히 검사하지
않았다. 코디네이터가 확인한 초기 구현의 sender 유지 후 thread join 교착,
동시 최초 room 생성·pooled advisory guard, 임의 연결의 persist 인가, 정확한
복원 bundle/expected tail 조건 누락은 수정 대상이다. 같은 검증된 Composer
process를 taskb149a7105ddb/ctx926f7177dd39로 재사용하여 보완·실제 회귀를 맡겼다.
root collab_product DB 검사 등록95af8b5만 명시적으로 cherry-pick 허용했다.
현재 통합95af8b5는 미수락이며 원격 push/새 PR 전 관련 결함을 해결한다.
UI ctx259ebcae441c는 독립 구현을 계속하고 전체 E2E는 backend 보완 뒤 실행한다.

코디네이터는 실제 collab_product 검사에 독립 x64/ARM64 CI job을 추가했다.
기존 PostgreSQL 인가 검사는 helper 빌드를 기다리지 않는다. 새 job은 production
worker만 별도 target/cache에서 빌드하고 정확한 FVOCI_COLLAB_ENGINE을 전달한다.
architecture/toolchain/lock/features/source를 캐시 키에 포함했다. actionlint1.7.12
exit0; 아직 push 전이므로 새 job의 원격 실행·제품 수락은 미완료다.

추가 미수락 통합 a2ccfb8: Composer e826aa7의 정확한 bundle Load/Snapshot 사전
검사와 expected_tail_seq를 보존했다. 보고된 PG/helper8검사는 부분 증거이며
room 슬롯/Starting/eviction 및 수신자 인가 결함, 필수 실패 검사가 남아 있다.
기존 Composer 재사용은 agent_readiness timeout(ctx4234b6e6b44a, 입력 미전달)로
실패했다. 완료된 이전 터미널을 공식 release한 뒤 동일 task47d50ab67905를
새 검증 Composer2.5 ctx5e765b78857a로 재시도했다. room lifecycle 수정·결정적
검사만 좁혀 소유권을 배정했고 원래 worktree/커밋을 보존했다.
Fable5.1medium taskbe94f25835c0/ctx6123a6f89afe는 고정 a2ccfb8의 durability
읽기 전용 검토 중; 요청·유효 모델과 turn 시작 확인. quota/Opus 전환 없음.
React Grok ctx259ebcae441c는 동일 소유권으로 진행 중. 새 통합 원격 PR/CI는
아직 시작하지 않았고 b706668의 Actions 문법 검사만 성공했다.

React 제출647a99b를 efd0005로 통합했다. 실제 원본 FvociEditor/schema/session
재사용, worker의 fresh npm ci 후 typecheck/web33/editor11/build 성공 보고.
전체 실제 협업 E2E는 e2e-pending에 있으며 미실행이다. postinstall의 재귀 설치와
React 모듈 삭제를 없애는 정상 패키지 해석 보완을 Grok task97ca3bc92c7d/
ctx602809de1bf8에 배정했다(동일 UI/editor 파일 소유권). 이후 UI 수락 전에는
connected/unsyncedChanges 기반 「저장됨」 배지를 durable persist ack와 분리하고,
기존 pending E2E의 순차 입력/약한 삭제 assertion을 실제 동시성·fresh crash
검사로 보완해야 한다. 이 제출은 동시편집 제품 수락을 뜻하지 않는다.

고정 a2ccfb8의 Fable 정적 검토 파일은 /tmp/fvoci-collab-live-durability-review-a2ccfb8.md.
SyncStep1 누락, primary256-op cap, readonly-first→writer Load 재사용, sticky persist
실패, 불명확 commit 뒤 stale state, commit 후 Apply 실패 시 peer 누락을 지적했다.
실제 Fable5.1medium 검토였으나 완료 전송에 빈 `orca`를 써 유효 worker_done이
없었다. 최종 transcript·빈 tool 출력을 확인하고 공식 worker-stop으로 종료했다.
Orca dispatch ctx6123a6f89afe는 stopped이며 검토 수락/성공 settlement로 표시하지 않는다.
quota/Opus 전환은 없었다. 지적은 후속 수정 입력으로 보존하고 수정 SHA를 다시 검토한다.

코디네이터 판단: poll 간격의 권한 cache로 철회 보장을 약화하지 않는다. 매 admission은
전체 현재 tail+candidate를 검사하므로 마지막 검증이 전체 당시 bundle을 포함한다.
시간 보장은 그 host/load 시점에 한정한다. persist 성공에 compaction/tail 비우기는
필수 조건이 아니며 이미 durable한 prefix와 compaction 건강 상태를 분리할 수 있다.
추가 결함: validate_snapshot_only가 빈 candidate를 tail에 넣어 항상 malformed가 된다.
기존 production helper(SHA256 686dd31f75452dfb51d1d5dee626e5077e3222ac3a8c2d59716df545fa99a96d)에
u32LE JSON frame으로 Load(snapshot_b64=AAA=,tail_b64=[])→ok(2ms),
동일 Load tail_b64=[""]→malformed tail[0]:empty updateV1(1ms)를 재현했다.
두 작은 child는 EOF 종료/회수; DB·전체 제품 검사 성공으로 확대하지 않는다.
통합 efd0005 cargo fmt --check exit1(0.29s): hub 빈 줄과 product assert 서식,
현재 worker 범위에 수정 요청했다. source/RLS·UI 제품 수락은 계속 보류다.

Grok packaging 제출2fa4a75를 d732c80로 통합: npm install-links=true/lock으로
재귀 postinstall·React 삭제를 제거했다. clean snapshot 설치 web5.55s/editor3.29s,
web typecheck3.23s/build4.19s, 네트워크 차단 unshare --net에서 web34/editor11
검사1.67s 성공 보고. React/Yjs 동일성 회귀 포함; 원격 검증은 아직 미실행.
ctx602809de1bf8 release 후 새 Grok taskba64ce66469d/ctxd56261e6dc69에
동기화와 durable persist ack를 구분하는 UI·삭제-only/늦은 ack 회귀를 배정했다.
coordinator Web CI/prepare는 명시적 editor npm ci 및 web/editor 단위 검사를
연결했고 bash -n/actionlint/diff --check exit0. 전체 협업 E2E와 수락은 미완료다.

미수락 협업 통합 d5aec64 (main ba19932 기반): Composer a2ce88b를 e569a8a로
통합한 뒤 코디네이터가 caller 취소와 독립적인 startup 소유권, eviction의 Closing
유지, shutdown 시 eviction/start task join을 보완했다. 테스트 hook은 문서별로
격리하고, 취소·최대 room 회수는 새 hub 생성 없이 같은 hub에서 검증한다.
production helper 별도 target build 9.35s; 실제 격리 PostgreSQL에서
`FVOCI_COLLAB_ENGINE=$PWD/crates/collab-engine/target/debug/collab-engine scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test collab_product -- --test-threads=4`
15/15, skip0, cold root compile41.28s/본문10.83s. fmt/check/clippy(all-targets,
db-tests, locked/offline) 및 lib24(본문5.59s) 성공. 최초 check는 std MutexGuard의
await 경계에서 Send 오류, 최초 fmt는 제출 engine_bridge 서식 오류였고 수정했다.
이는 lifecycle·기존 smoke 범위이며 persist/crash/실제 React 협업 수락이 아니다.

Grok bc321fe를 f9cc7cd로 통합: durable ack 기반 badge·회귀 web46/editor11,
typecheck/build는 워커 성공 보고. 검토에서 같은 provider 재접속이 실제 connection
세대를 바꾸지 않는 빈틈을 확인하여 task602cc4bdbbc3/ctx35da0fcbd36e에 후속 수정
배정했다. 소유권은 UI session/ack/tests에 한정한다. 서버 lifecycle 소유권은
코디네이터에서 다음 Composer durable persist/reconnect task로 넘긴다.
Fable의 실제 주간 한도 시에만 검증된 Claude Code Opus5.5 medium 대체를 적용하며,
현재까지 quota 오류나 Opus 실행은 없다. 독립 검토의 기존 차단 지적은 미해결이다.

다음: 검증한 코드 묶음을 Draft PR로 원격 CI에 제출하고(수락/머지 아님),
서버 snapshot-only/persist barrier/helper 수명/reconnect 및 UI generation 수정을
통합한다. 수신자 현재 인가·backpressure·실제 두 React 클라이언트·fresh-client
crash 복원 검증은 남아 있다. /collab은 명시적 FVOCI_COLLAB_ENGINE 설정이 없으면
활성화되지 않는다. 협업 지원 완료나 OS IME 검증 완료로 표시하지 않는다.

원격 진행: Draft PR #7 https://github.com/AISFlow/fvoci/pull/7,
HEAD759ebfe2bc1bf3abce0bf16f8f515fc71c7dc1d6/baseba19932/
GitHub merge6823f0fe3e90c5eeab27ac932cc68e24315d2c35.
Rust35933136653/Web35933136821/NativeEngine35933136717/Documents35933136856
실제 jobs 시작 확인; 아직 성공/수락/머지 아님. 기존 기본 CI8개와 새 collaboration
x64/ARM64 2개가 독립 실행하며 branch push 중복 실행은 없다.

진행 task: Composer taskae5936c89d8d/ctxb50a9487aa87는 rust-collab-room
base924fdc6에서 room/validation/engine_bridge/y_sync/product검사 단독 소유,
현지 무거운 검증도 이 worker 한 묶음에 배정. Grok task602cc4bdbbc3/
ctx35da0fcbd36e는 rust-collab-react UI ack/실제 reconnect 세대 수정 소유.
Fable task8dae21bc41e6/ctx7a57eaf493fc는 고정 d5aec64 lifecycle 읽기 전용 검토,
Claude Code Fable5.1 medium 요청/유효 설정과 turn 시작 확인. quota 대체 없음.

후속 차단 근거: src/main.rs가 hub.shutdown을 호출하지 않아 explicit hub 검사와
실제 프로세스 종료 보장을 구분해야 한다. src/collab/awareness.rs의 chunk별
길이 framing은 실제 y-protocols의 count→clientID→clock→JSON string과 다르다.
원본 collab.ts205–262의 user 중첩/허용 cursor·block·title/null 계약과 현재
sanitize 결과도 다르며 generation 증가/삭제 tombstone 전달이 필요하다.
compat/fixtures/hocus-wire.json의 실제 provider awareness fixture를 재사용하여
제품 parser/registry의 독립 회귀를 추가할 다음 좁은 작업으로 남긴다.

PR7 첫 CI 결과: HEAD759ebfe에서 fast·collaboration x64/ARM64·Web·rhwp2·
engine2 성공, postgres x64/ARM64 실패(전체10 jobs 중8성공2실패).
실패는 `append_rejects_state_budget_exhaustion`이 64행 fixture에 expected_tail=0을
전달해 StateBudgetExceeded 이전 StaleCutoff를 받은 것. 제품 조건을 약화하지 않고
fixture expected_tail을64로 맞춘 e245122의 실제 PostgreSQL 단일 회귀가 성공했다
(`scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test collab_integration append_rejects_state_budget_exhaustion -- --exact`,
compile4.95s/본문2.63s, 1passed/21filtered/0ignored). 전체 원격 gate는 재실행 필요.
Web 성공은 현재 등록된 기존 사용자 흐름이며 e2e-pending 협업 수락을 포함하지 않는다.

PR7 HEADdf251639e173b550ee9a7ea579a9b2d0c09db90f의 원격10 jobs 모두 성공.
Rust35933621809: lib24/wire11, 각 아키텍처 실제 collabDB22+기존DB66+document13,
product15(본문 x64 19.03s/ARM64 14.01s), skip0. Web35933621783: web46/editor11,
기존 React13(53.6s). Documents35933621780: 각52/production52,
Engine35933621789: 각test38/production35. 이 결과는 후속 로컬 코드의 증거가 아니다.

Fable task8dae21bc41e6/ctx7a57eaf493fc는 유효 worker_done 후 release했다.
/tmp/fvoci-collab-lifecycle-review-d5aec64.md: F1 미가입 leave 인원 감소(High),
F2 main 종료 연결 누락(High), F3 startup panic 슬롯 유실(Medium), 잠금/종료
대기와 추가 경합 검사 권고. 코디네이터 b907697은 실제 member HashSet·unknown
leave/frame 무시, startup unwind 정리, 실제 서버 종료 hub join을 구현했다.
clippy alltargets/db-tests 성공만 확인했으며 새 runtime 회귀/독립 재검토는 미완료.
naive timeout으로 shutdown 소유 future를 버리는 조치는 하지 않았다.

Grok UI 803855d/844f36c를 786c7db/8dd30cd로 통합했다. 동일 provider 재접속과
room/connection 세대별 callback을 분리하고 연결 상실 시 실제 persistNow promise를
reject/리스너·timer 정리한다. 실제 provider emitter 기반 web56/typecheck는 워커
성공 보고, 새 통합 원격 검증 전이다. ctx02be004c9563 release 완료.

Composer5737f392a345e30c5385cc8375fef9888d533a57은 제품21검사 성공 보고를
제출했으나 미통합·미수락이다. 보고의 placeholder full SHA는 버리고 실제 Git SHA를
확인했다. primary reload 실패를 무시하고 healthy로 표시하여 stale snapshot으로
새 durable tail을 compact할 위험, 두 번째 readonly Load, malformed empty update,
불일치 receipt/tail 복구가 남아 동일 작업자 task86f04fca1c51/ctx5e5b80a00614로
후속 수정·실제 실패 회귀를 배정했다. b907697 소비와 F1/F2/F3 회귀도 명시했다.
현재 heavy bundle은 이 Composer가 소유한다.

Grok task7324576c22d4/ctxf628dd69a966: 새 Orca task worktree rust-collab-awareness,
base8dd30cd, 허용 src/collab/awareness.rs와 inline tests만. 원본 provider framing,
중첩 verified user/allowlist, clock·generation·null tombstone·자원 제한을 구현한다.
Composer room/제품검사 파일과 겹치지 않는다. 두 쓰기 워커가 진행 중이며 기존
모든 커밋/worktree/retained 자원을 보존한다. 다음 명령은 해당 두 dispatch의
`orchestration check` 결과 확인, 제출 SHA 검토 후 하나씩 통합·관련 검사다.

PR7 후속 통합 코드 b06a9a849218fc610c0c8d0457f51a41fc5c397d (미수락):
Composer5737f39/ccbaf0b/aff84e8와 Grok79a26a5를 순차 통합했다. helper 재생성,
실패한 Load의 unhealthy 상태 유지, snapshot-only 검사, readonly 재접속,
실제 y-protocols awareness framing·verified user·tombstone을 반영했다.
코디네이터는 receipt 없는 tail 성공 추정을 제거하고 dirty/unloaded primary 복구,
malformed empty update 연결 거부, persist의 poisoned/in-flight 거부를 보완했다.

통합 로컬 검증: `cargo check --locked --offline --all-targets --features db-tests`,
`cargo fmt --check`, `cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings` 성공.
처음 clippy는 제출된 테스트의 네 경고로 실패했으며 해당 표현을 수정했다.
`cargo test --locked --offline --lib collab::awareness` 8/8, skip0.
`FVOCI_COLLAB_ENGINE=$PWD/crates/collab-engine/target/debug/collab-engine scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test collab_product -- --test-threads=4`
27/27, skip0, compile6.72s/본문42.63s. 뒤이어 종료 barrier와 실제 멤버 수 검사를
강화한 `collab_lifecycle_` 선택 검사는 9/9, compile4.35s/본문8.18s.
마지막 변경은 테스트 표현/서식이며 최신 통합 전체 원격 gate는 아직 대기다.

직전 원격 HEAD70088ea의 10 jobs 성공(Rust35934513996/Web35934514155/
Documents35934514047/Engine35934513998). 새 코드의 원격 증거로 재사용하지 않는다.
PR7은 Draft/open이며 전체 협업 수락·머지하지 않았다. Composer ctx5e5b80a00614와
Grok ctxf628dd69a966는 유효 완료 보고 후 release, 커밋/worktree는 보존했다.

다음 진행 (동일 Run run_b01d432a9dee):
- Composer task_f5025d90f3a7/ctx_477df6e00c12, rust-collab-delivery,
  baseb06a9a8, transport/room/config/hub/product검사 소유. 현재 recipient 인가,
  idle 철회·세션 만료, bounded socket/backpressure와 실제 DB 회귀. 현지 heavy bundle 소유.
- Grok task_11abb7be7bc5/ctx_dc9df3677ffa, rust-collab-ui-acceptance,
  baseb06a9a8, e2e-pending 협업 검사만 소유. 실제 React 두 클라이언트와 fresh-client
  crash 복원 검사 구현; heavy 실행은 Composer 종료 후 배정한다.
- Fable task_81309013842e/ctx_6b5d9cb29a2e, 고정b06a9a8 읽기 전용 검토.
  요청/유효 Claude Code claude-fable-5-1 medium 일치. 실제 주간 한도 오류 없음,
  Opus5.5 medium 전환 미실행. 한도 도달 시에만 실제 지원/유효 설정 확인 후 대체한다.

미완료: 수신자 현재 인가·slow-client 처리, SIGTERM helper 회수, 실제 두 UI의
권한 철회·삭제·fresh-client crash 복원, REST 본문 projection 정합성, 구조 보존.
제품 협업은 명시적 helper 환경 설정 없이 비활성 상태를 유지한다. OS IME 미검증.
다음 명령: 위 dispatch의 `orchestration check`로 질문/제출 처리 후 고정 SHA 검토,
각 제출 하나씩 통합·관련 검사·PR CI 확인. 저장소 밖 소유 불명 자원은 건드리지 않았다.

PR7 HEAD9b3b66ea3a408a07ea20a202c28aa2fb3ce24ab6의 10 remote jobs 모두 성공.
Rust35935717526: lib32/wire11, product27 각 플랫폼(x64 54.62s/ARM64 44.43s),
실제 DB22+66+13 각 플랫폼, skip0. Web35935717458/Docs35935717461/
Engine35935717542 성공. Web는 기존 등록 사용자 흐름이며 pending협업 E2E가 아니다.

Fable ctx6b5d9cb29a2e 검토 완료 후 release, 실제 Fable5.1 medium/주간 quota 오류 없음.
/tmp/fvoci-collab-review-b06a9a8.md: R1 malformed Step1 이후 dead primary의 healthy
flag로 후속 sync 정지(High), R2 확정 rollback 거부를 ambiguous commit으로 분류하여
전체 peer 종료(Medium-High), R3 join Load 실패 flag, R4 compaction 회복, R5 검사
강도, R6 readonly persist 지적. 수락 보류. Composer ctx477df6e00c12에 같은 소유
파일의 수정·회귀를 추가 배정했다. root에 Yrs parser를 연결하거나 서버 내부 조회를
무인가로 우회하라는 제안은 채택하지 않았다.

Grok ctxdc9df3677ffa의 3fc0c1b는 미통합 E2E 검사 초안이며 후속 미커밋 보완 중.
`apps/web/e2e-pending/collab-*.ts`, 기존 pending spec 및 `scripts/web-e2e-inner.sh`
소유권을 위임했다. 실제 child handle/process group 소유로 정상 SIGTERM과 강제
process-tree SIGKILL을 구분하고 fresh context의 복원을 검사한다. 단순 watcher나
PID 파일만으로 소유권을 주장하지 않는다. 아직 브라우저 수락 결과는 없다.

Composer의 새 saturation 선택 검사는 실패했고, full queue에 Close를 await하는
경로와 transport 이전 mpsc byte budget 누락을 후속 수정 중이다. timeout/retry를
늘리지 않는다. 해당 test 종료 및 worktree의 cargo/collab/rustc 부재 확인 후
현지 heavy slot을 Grok으로 이관했다(2026-09-24 00:06 UTC); Composer는 수정/빠른
검사만, Grok은 `FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh` 실행 담당이다.
이들은 여전히 진행 중인 task이며 수락된 제출로 표시하지 않는다. PR7 Draft 유지.

진행 보완 (2026-09-24 00:30 UTC, 통합 HEAD9b3b66e 불변):
Grok pending E2E642b7ff는 5 pass/1 fail/8 미실행(53.8s); offline 시나리오의
초기 연결 실패이며 RoomFull은 당시 로그가 없어 가설이다. 공유 문서 재사용
b4eeff4는 3 pass/1 fail/10 미실행(33.1s), 삽입/삭제 기대 실패. 기존 fixture
간 오염을 없애도록 시나리오별 독립 문서·소유 서버 recycle로 수정 중이다.
커서 이름표는 본문 DOM decoration으로 구분하고 실제 텍스트 정규화를 하지 않는다.
실패 trace가 EXIT cleanup으로 사라진 문제는 실행별 고유 보존 경로로 보완 중.
이 결과는 협업 수락이 아니며 변경은 아직 worker worktree에만 있다.

Composer ctx477df6e00c12는 실제 mpsc enqueue 전 바이트 예산, 독립 종료 신호,
close 코드, readonly 거부 이후 persist barrier, awareness generation/locale,
Fable R1–R6를 계속 구현한다. host32CPU/load0.56/available40.8GiB 관찰에 따라
CARGO_BUILD_JOBS=2와 소유 helper/DB로 한 번에 좁은 DB 회귀 하나를 허용했다.
전체 heavy bundle은 Grok 소유이며 전체 suite 병렬 실행을 허용한 것은 아니다.
두 task 모두 진행 중으로 소유권/기존 commit을 유지한다. 다음은 각 제출의
실행 결과 확인, 고정 SHA 통합과 Fable 독립 재검토다. PR7 Draft/open 유지.

Grok ctxdc9df3677ffa는 HEAD39a298792e7238aa4818d4bd14c75741bcd6f9bc를
`worker_done failed`로 제출하고 release했다. 미수락 pending E2E를 통합했으며
제품 성공으로 보지 않는다. 최신 실제 전체 실행(HEAD36c4901)은 exit1/71.231s,
7 pass 후 revoke 검사 실패(unauthorized 이후 editor가 unmount된 실제 UI에
기존 locator를 사용). 제출39a2987에는 locator 보완이 있으나 이후 전체 미실행.
presence 최초 실패의 trace에 WS frame이 없으므로 'pageB가 원격 awareness를
수신했다'는 워커 주장은 철회되었다. 이후 연결별 진단 attachment를 추가했다.
아티팩트 /tmp/fvoci-collab-e2e-fail.T2OqHQ, 앞선 presence는 ywHrXm.
코디네이터가 제출39a2987에서 Node wire/restart 검사4/4(skip0,96.7ms),
`apps/web/node_modules/.bin/tsc -p apps/web/e2e-pending/collab-tsconfig.json` 성공을
확인했다. shell syntax/diff check 성공. Native/browser 전체 수락은 아직 아니다.
heavy slot은 실제 잔여 프로세스 없음 확인 후 Composer ctx477df6e00c12로 반환했다.

Fable task26e08a380ab6/ctxd38f3f089237 파생 본문 경계 자문 완료·release.
실제 Fable5.1 medium, quota 오류/Opus 전환 없음. 고정b06a9a8/source3937952.
보고서 /tmp/fvoci-collab-derived-boundary-review.md: binary 정본 유지, loaded/clean
primary만 projection, writer_generation+tail_seq 정확한 fence, tail_seq0 seed 보존,
source version 유지, 인가·event 실패 rollback와 원본의 derived 실패 의미 확인.
검사 실행 없는 설계 자문이다. 보고서의 '설치 y-tiptap 부재'는 잘못된 범위 조회:
rust-collab-ui-acceptance/apps/web/node_modules에 실제 구현이 있으며 native
구현 워커는 그 고정 라이브러리와 독립 JS fixture를 반드시 대조해야 한다.
