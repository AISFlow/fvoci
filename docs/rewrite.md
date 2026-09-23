# FVOCI Rust 재작성

## 기준선 (2026-09-24)

- 대상 AISFlow/fvoci: 초기 `97a3fe61ede69390b78beaf2de8dd394ad49eed1`, 통합 브랜치 `fvoci/daggertooth`. 기존 미추적 starter 파일을 검토·보존하며 시작.
- 원본 fvoci/FVOCI main: `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`.
- 열린 PR #999: base `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`, head `393795261322b916e588043cf94feca999175843`, 미병합. 기능 조사 기준은 이 HEAD; main과 차이는 별도 검토.
- 원본은 비공개 별도 참조 clone. 공개 문서 서비스에 소스 전송 금지. 기존 원본 개발 checkout은 수정하지 않음.

## 현재 수락 지점

제품 기능 수락 없음. 환경 준비와 고정 SHA 계약 조사 중. Run `run_b01d432a9dee`.

| 기능 | 원본 근거 | 보존할 외부 동작·불변식 | 새 구현 | 검증 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·세션·인증된 프로필 변경 | PR HEAD에서 조사 중 | 활성 사용자, 정지 경합, 프로필/이벤트/감사 원자성 | 미착수 | 미실행 | 첫 수직 기능 |
| 협업·문서 처리 | PR HEAD에서 조사 중 | 저장 문서, 실제 provider, 한글/이모지, 철회·복원 | 미착수 | 미실행 | 조기 호환성 위험 검사 |
| 나머지 제품 기능 | 실제 라우트·UI 목록 조사 예정 | 원본 기능·보안·데이터 계약 | 미착수 | 미실행 | 인증 확장, 워크스페이스/프로젝트/태스크/문서, 첨부/검색/알림/연동, MCP/CLI/운영/백업 |

## 설계·자원 결정

작은 단일 Rust 서버 + PostgreSQL로 시작한다. worktree별 target, 실행별 DB/역할/스토리지, port 0 사용. 빠른 순수 정책 검사와 실제 DB 검사를 분리한다. 동시 쓰기 워커 최대 2개, 무거운 검증 한 묶음. 원본 다중 DB 미완료 범위와 재작성 미구현을 구분하며 첫 단계 성공을 전체 완료로 표현하지 않는다.

## 재개

AGENTS.md → .agents/environment.md → Orca Run task-list → 이 문서 → git status/worktree와 실제 프로세스를 대조한다. 진행 중인 작업을 중복 배정하지 않는다.

## 첫 구현 착수

공통 기반 d692134에서 Composer task_3377f29385d5 / ctx_6f56b812c5f0이 독립 rust-profile-slice worktree에 첫 기능을 구현 중이다. 원본 프로필은 PATCH /api/v1/auth/me, 엄격한 givenName/familyName/locale/timezone/weekStartsOn/textScale 입력, fvoci_session 쿠키, sessionUserOutput 반환. 정지 경합은 401 authentication_required이며 본문과 이벤트가 남지 않아야 한다. 원본 이벤트는 user.name_updated (감사 비활성); 이번 요구에 따라 감사도 함께 원자 기록하는 의도적 차이를 둔다. 프론트 계약 정본은 Rust DTO로 두고 클라이언트 생성 여부는 통합 시 확인한다.

## 범위 보존 목록

아래 원본 근거는 모두 고정 PR HEAD 기준이다. 목록은 지원 선언이 아니다.

| 기능 | 원본 근거 | 보존할 외부 동작·불변식 | 새 구현 | 검증 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 인증 확장·PAT·OIDC·MFA·사용자 생명주기 | packages/contracts/src/routes.ts auth/me/admin, core/auth.ts | 세션 폐기, 범위, 마지막 관리자, 탈퇴/복구 | 재작성 미착수 | 미실행 | 첫 로그인 외 전체 |
| 워크스페이스·멤버십·그룹·인가 | routes.ts workspaces/groups/apiTokens, server domains/workspaces | 현재 권한, 철회 경합, 테넌트 RLS·풀 컨텍스트 | 재작성 미착수 | 미실행 | 첫 workspace slice에서 실제 앱 역할 RLS 수락 |
| 프로젝트·태스크·일정 | server domains/projects/tasks, routes.ts ics/holidays | API·공유/멤버 권한·일정 의미 | 재작성 미착수 | 미실행 | 전체 |
| 문서·위키·댓글·공유·리비전 | server domains/documents/comments/share | 저장 형식·리비전·읽기/쓰기 권한 | 재작성 미착수 | 미실행 | 전체 |
| 협업 | server 협업 구현, editor/package.json | Hocuspocus 4.6.0, Yjs13.6.32, Tiptap3.31.3, 두 클라이언트·철회·재시작 | 재작성 미착수 | 조사 중 | CRDT 호환과 provider 호환을 별도로 검증 |
| 첨부·local/S3·추출·썸네일 | server domains/attachments, packages/storage | 다운로드 인가·지원 형식·취소/자원 제한 | 재작성 미착수 | 조사 중 | native 대체 실증 필요 |
| 검색·색인·AI | server domains/search, packages/search, routes.ts ai | 검색에서도 인가·철회·색인 복구 | 재작성 미착수 | 미실행 | 전체 |
| 알림·메일·webhook·연동 | server domains/notifications, packages/jobs, routes.ts github/webhooks | outbox·커밋 후 전달·중복/재시도 | 재작성 미착수 | 미실행 | 전체 |
| 동의·감사·사용권 | routes.ts legal/auth.consents/admin.audit, packages/ee | 동의 gate·증거·서명·권한 | 재작성 미착수 | 미실행 | 프로필 감사 강화 외 전체 |
| MCP·CLI·설치·백업·복구 | server src/init.ts/backup.ts/doctor.ts, 제품 MCP | 프로토콜·오류·복원·운영 취소 | 재작성 미착수 | 미실행 | 첫 설치 외 전체 |
| SQLite/libSQL/Turso 및 이관 | PR999 packages/db | 원본 제공 범위와 목표 구분 | 재작성 미착수 | 미실행 | PG 우선; 원본 전체 다중 DB 완료로 간주하지 않음 |
| 프론트엔드·배포·ARM64 | apps/web, packages/editor, 배포 리소스 | 한국어·접근성·편집 흐름·정적 자산 | 재작성 미착수 | 미실행 | 최초 실제 HTTP slice 이후 연결; 이관/운영/ARM64 지원 미선언 |

## PR 및 원본 회귀 확인

- 사용자가 대상 PR #1 (`fvoci/daggertooth` → main)을 직접 열었다. 새 PR을 생성하지 않는다.
- PR999 현재 CI35877360079: PG16/17/18 database-contract, image-tests, native standalone-arm64 성공; test의 `bun run test:pkg --affected`와 집계 report 실패. 원본 문서의 이전 성공 수치를 이 CI의 성공으로 대체하지 않는다.
- 원본 확인: 설치 GET/POST /api/v1/setup, 성공201 {userId,workspaceId}, 재설치404 instance_setup_already_completed. 로그인 POST /api/v1/auth/login, 성공200 {userId}, 누락/오류/정지401 invalid_email_or_password. 설치는 실제 첫 workspace/owner도 원자 생성해야 한다.
- 협업 위험 probe: Grok task_1532bb645f74 / ctx_00b1baf38504, rust-compat-probe worktree, base31b6790, 단독 소유 compat/**. 저장 updateV1과 Hocuspocus 프레임 호환을 따로 확인하며 실제 UI/권한 검증 미실행을 감추지 않는다.
- 의존성 공식 확인: docs.rs axum0.8.9/SQLx 및 crates.io 버전 메타데이터(axum0.8.9 MIT/Rust1.80, SQLx0.8.6 MIT OR Apache-2.0, Tokio1.47.1 MIT/Rust1.70, Serde1.0.228 MIT OR Apache-2.0). 선택 실제 버전은 Cargo.lock에서 고정·검증한다. 이 조회는 벤치마크가 아니다.
