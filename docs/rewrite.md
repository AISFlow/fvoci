# FVOCI Rust 재작성

이 문서는 현재 기능·수락·잔여 작업의 정본이다. 운영 규칙은 `AGENTS.md`, 실행 환경은
`.agents/environment.md`, 설치·실행 절차는 `RUNNING.md`에 둔다. 2026-09-25 이전의 PR별
진행 일지·검토 종결·핸드오프 기록은 이 파일의 git 이력에 그대로 있다
(`git log -p -- docs/rewrite.md`, 압축 직전 main `c6dd5ef7852338487d9d88ae1f7c70d029d29f2b`).

## 1. 기준선 (2026-09-24)

- 대상 AISFlow/fvoci: 초기 `97a3fe61ede69390b78beaf2de8dd394ad49eed1`.
- 원본 fvoci/FVOCI main: `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f`.
- 원본 PR #999 HEAD `393795261322b916e588043cf94feca999175843`(미병합)이 기능 조사 기준이다.
  원본 최신 변경을 따라가며 목표를 늘리지 않는다.
- 원본은 비공개 별도 참조 clone(읽기 전용)이다. 공개 문서 서비스에 원본 소스를 보내지 않는다.

## 2. 현재 수락 지점

전체 재작성은 **부분 구현**이다. Orca Run `run_b01d432a9dee`.
최신 수락 main `a2a1593131125d567bc21b9f25ed6a46f2db1479` (PR #1–35).

| PR | merge | 범위 | 수락 근거 |
| --- | --- | --- | --- |
| #1 | fe30bd1 | 설치·로그인·세션·프로필 | 실제 PG/HTTP, 독립 검토 |
| #2 | 1fc8af3 | rhwp native HWP/HWPX 본문 helper | x64/ARM64 native CI, 검토 |
| #3 | 2fb61f9 | CI: ARM64 병렬·캐시 범위 | 원격 CI |
| #4 | 0617e7f | 첫 workspace API·React | 앱 역할 DB, React, 검토 |
| #5 | 421e70b | 위키 생성/조회/metadata·React | 앱 역할 DB, UI, 검토 |
| #6 | ba19932 | 협업 codec/DB/native engine | lifecycle·product·projection, 검토 |
| #7 | 9c39532 | 실제 /collab ↔ React 2UI (opt-in) | 2UI E2E, 검토 |
| #8, #9 | 125ef25, 59b6ecd | 추출 process client·취소·부모 사망 격리 | native CI, 검토 |
| #10 | 0582c29 | 인가된 위키 첨부(local) | 첨부 DB·React, 검토 |
| #11 | 006943b | 첨부 HWP/HWPX 추출 job | 추출 DB, 검토 |
| #12, #14 | 3fa41bc, b5ab024 | 위키 저장·선택 진단, 본문 oracle | 협업 E2E |
| #13 | b87a908 | 프로젝트·태스크 생성/목록/상세/lookup·페이지네이션·React | 13 CI, project35/task30, Opus 검토+delta |
| #15 | fe624aa | 종료: 신호 선등록·listen 준비·HTTP drain 수락 barrier | collab_shutdown 7×2, 검토+delta |
| #16 | 472c905 | `--grant-app-role` 단일 트랜잭션·definer PUBLIC 회수·migration 고정 | db70×2, 검토+delta |
| #17 | f4a2cad | 컨테이너 설치 산출물·install-smoke | smoke x64/ARM64, 검토+delta |
| #18 | 5088c25 | 협업 Delete/Backspace native caret 단일 소유 | 협업 E2E 20, 검토+delta |
| #19 | 0b83f33 | 태스크 PATCH/move/trash/restore·WIP·반복 회차·엄격 ISO 날짜 | task64, 검토+delta×3 |
| #20 | b13dae8 | 서버 기동 migration 제거·스키마 게이트·앱 URL만 보유 | db75, smoke, 검토+delta |
| #21 | c6dd5ef | 워크스페이스 멤버 목록·초대 생성/미리보기/수락·좌석 한도 | invitation11, invite E2E, 검토+delta |
| #22 | f570918 | 문서: rewrite.md 압축·상시 자문 기록 | CI |
| #23 | 66ebef2 | 위키 문서 생명주기(rename/move/sort/trash/restore)·단일 `document_permission` | document19·document_collab_lifecycle3, 검토+delta×2 |
| #24 | 9a7b669 | 협업 caret: awareness 전용 갱신이 native caret을 덮지 않게(읽기 전용 gate) | caret 60/60·pending 23, 검토+delta |
| #25 | 2473d8f | 리비전 목록/생성/조회/복원(room actor forward 복원) | revision8, 검토+delta |
| #26 | 1c50016 | 태스크 편집 React UI(원본 HierarchyForm) | task-edit/project-task E2E, 검토+delta |
| #27 | 107524f | 협업 ACL poll이 틱마다 실행(2배 철회 지연 수정)·collab_product harness | collab 전체, 검토 |
| #28 | 54adee9 | 댓글(문서 XOR 태스크)·스레드·resolve·반응·문서 댓글 UI | comment13·comments E2E, 검토+delta |
| #29 | a46ce76 | 검색 기반: 원본 동등 text(Porter/초성/NFKC)·Meilisearch client·scoped key·설치/CI | search_meili2·install smoke, 검토+delta |
| #30 | 8b858b4 | 워크스페이스 검색 API·PG hydrate 보안 경계·검색 UI | search_query·leak matrix, 검토+delta |
| #31 | f0b132d | outbox relay: 임대 cursor·PgOnly/External 전달·dead letter·requeue·xid epoch fail-closed·`--recover-outbox`(restore에서 실행) | outbox21·db75·shutdown7, 검토+delta×4 |
| #32 | a3cee11 | 컨테이너 설치 백업·복구(pepper fingerprint 검사) | backup-restore-smoke x64/ARM64, 검토 |
| #33 | 626dc09 | 문서: #22–30 기록 | CI |
| #34 | f0f84f5 | 개인 API 토큰(생성/목록/철회·Bearer·원본 허용 route만·scope 도메인 규칙) | api_token8·api-token E2E, 검토+delta |
| #35 | a2a1593 | 검색 색인: outbox `search-index` 소비자·첨부 text chunk(014)·`--rebuild-search`(restore에서 실행) | search_index8·복구 후 검색 smoke, 검토+delta |

검증 기준: 각 PR의 최종 HEAD에서 원격 Rust/Web/Native/Container install 워크플로가 실제 실행되고
(PG suite는 `--no-fail-fast`), 별도 세션의 Opus 5.5 medium 검토 차단 사항이 해소된 뒤 기대 HEAD로
squash merge했다. 세부 run id·검토 보고서는 각 PR 코멘트에 있다.

진행 중(미수락): 협업 room 용량(`rust-collab-engine-capacity`; 200 room probe는 미통과 — 검증 목표를
64 room·p95 ≤ 300 ms·writer 손실 0으로 낮추고 validator 용량/폴링/PG 연결 제약 수정 중),
태스크 담당자·라벨(`rust-task-assign-labels`, migration 015).

## 3. 기능 대응표

상태: 수락(main 반영·검증) / 부분(일부 경로 수락) / 진행 / 미착수. "남은 차이"는 전체 포팅 완료 전
해결하거나 사용자가 승인한 차이로 기록해야 하는 항목이다.

| 기능 | 원본 근거 | 보존할 불변식 | 상태 | 증거 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·로그인·세션·프로필 | identity/routes.ts, core/auth.ts | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | 부분 | #1 | OIDC·MFA·비밀번호 재설정·계정 생명주기·설정 기반 비밀번호 정책 |
| 워크스페이스 | domains/workspaces | 현재 역할·철회 경합·RLS·풀 컨텍스트 | 부분 | #4 | counts·groups·workspace 삭제·workspace/guest/storage quota |
| 멤버·초대 | invitation.ts, quota.ts, consent.ts | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | 부분 | #21 | 메일/SMTP, 수락 시 MFA/OIDC, legal consent 428, pending 목록/철회 API, 알림 설정 기본값, 계정 삭제 시 pending 정리, 다른 E2E의 SQL fixture 멤버 |
| 그룹·권한 통합 | policies.ts effectivePermission, project/document_members(user XOR group) | 리소스별 단일 권한 함수 | 부분 | #23(document_permission), #28·#30이 재사용 | groups 전체, 문서 멤버(document_members)·guest 문서 부여 |
| 프로젝트 | domains/projects | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | 부분 | #13 | 프로젝트 홈·프로젝트 문서·복제·lead 선택 UI |
| 태스크 | domains/tasks, core/task.ts, workflow.ts | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | 부분 | #13, #19, #26 | 담당자·라벨·마일스톤·의존성(반복 회차 assignee/label 복사, `dependency_contradiction`)·activity feed·태스크 댓글 UI·검색 가능한 parent 선택 |
| 일정·ICS·휴일 | routes.ts ics/holidays | 일정 의미 | 미착수 | — | 전체 |
| 위키 문서 | domains/documents, core/document.ts | 현재 문서 권한·트리 잠금 순서 | 부분 | #5, #23 | 공유·내보내기·확장 문서 기능, trash 영구 삭제 GC |
| 리비전 | documents/revisions.ts, core/revision.ts, collab applyRestore | 복원은 room actor의 forward system update, durable 후 broadcast | 수락 | #25 | 리비전 routes의 `document_permission` 통합, writer-stale 후 committed_loaded 재설정(후속) |
| 댓글 | comments/routes.ts, core/comment.ts | 문서 XOR 태스크·부모 활성·권한 | 부분 | #28 | 태스크 댓글 UI, 그룹 멘션, 이중 DELETE 중복 이벤트·resolve 경합·동시 trash된 태스크 댓글(후속), 프로젝트 문서 댓글 |
| 협업 | domains/collab, React/Tiptap | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | 부분(opt-in) | #6, #7, #18, #24, #27 | 실제 OS IME, 기존 데이터 전체 호환, **room 용량**(room당 helper 프로세스·기본 4 room/서버, 원본 대비 제약 → 진행 중, 위 §2), opt-in 해제 조건 |
| 첨부 | domains/attachments, packages/storage | 부모 권한·원본 bytes·원자 완료·취소 | 부분 | #10 | S3, 썸네일, 태스크 등 다른 부모, 중단 업로드·임시 파일 GC, quota·디스크 부족 의미 |
| HWP/HWPX 추출 | 원본 추출 경로, rhwp e8800c8 | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | 부분 | #2, #8, #9, #11 | 검색 색인·썸네일 연결, lease 만료·재시도 결과 게시 경계 |
| 검색·색인·AI | domains/search, packages/search | 검색에서도 인가·철회·색인 복구 | 부분 | #29, #30, #35 | 전역 GET /search, 의미(벡터) 검색, 댓글 검색 hit, 검색 E2E, partial 추출 chunk, 협업 편집마다 첨부 chunk 재색인(쓰기 증폭) |
| 알림·outbox·메일·webhook·연동 | domains/notifications, packages/jobs | 커밋 후 전달·중복/재시도 | 부분 | #31(relay), #35(첫 소비자) | 알림·메일·webhook·연동 소비자 전체, requeue 운영 API/UI |
| 공유·즐겨찾기·최근·태그·컬렉션 | 해당 routes | 공유 링크 권한 | 미착수 | — | 전체 |
| 동의·감사·사용권·관리 | legal, auth.consents, admin.audit, packages/ee | 동의 gate·증거·권한 | 미착수 | 감사 원자 기록만 | 전체 |
| 제품 MCP·CLI·백업·복구 | init.ts, backup.ts, doctor.ts, MCP | 프로토콜·복원 | 미착수 | — | 전체 |
| 설치·배포 산출물 | infra/app, compose | 비특권 실행·migrate/grant 분리·helper 포함 | 부분 | #17, #20, #29, #32, #35 | 운영 TLS/secure cookie 안내, 업그레이드 경로 |
| 추가 DB·플랫폼 | PR999 packages/db (SQLite/libSQL/Turso) | 원본 제공 범위와 목표 구분 | 미착수 | — | PG 우선; 원본 다중 DB를 완료로 간주하지 않음 |
| 프론트엔드 | apps/web, packages/editor | 한국어·접근성·기존 흐름 | 부분 | 각 PR E2E | 위 미착수 기능의 UI 전체 |

## 4. 유지하는 결정과 의도적 차이

- 작은 단일 Rust 서버(axum·SQLx) + PostgreSQL, 협업은 Yrs native engine, HWP는 rhwp native helper.
  프레임워크·CRDT·HWP 구현체를 다시 비교하지 않는다.
- DB 역할: 소유자 URL은 `fvoci-migrate`와 `--grant-app-role`만 사용한다. 서버는 `DATABASE_APP_URL`만
  받고 기동 시 스키마 버전(`schema_migrations`, 앱 역할 SELECT 전용)이 컴파일 버전과 같아야 한다(#20).
  권한 적용은 단일 트랜잭션이며 수락된 migration 파일은 SHA-256으로 고정한다(#16).
- 협업은 검증된 opt-in 범위만 수락했다. 신뢰하지 않는 문서 parser는 요청 처리 프로세스와 분리된 helper다.
- 의도적 원본 차이: 프로필 변경 감사 기록 추가, 세션 철회 중 쓰기 차단 강화, 429 계약 정합,
  엄격한 ISO 날짜(원본 `z.iso.date()`와 동일 수용 집합).
- 검색은 사용자 결정으로 원본과 같은 Meilisearch를 쓴다. 서버는 index 범위 scoped key만 받고(마스터 키는
  init만), Meili 필터는 recall 최적화이며 보안 경계는 PG hydrate다. Meili가 없으면 서버는 기동하고 검색만
  503을 낸다(키 거부 401/403만 기동 실패).
- 협업 helper는 신뢰할 수 없는 Yrs 디코더 격리 때문에 room당 프로세스를 유지한다(I1–I5). 용량은 개수 상한
  대신 메모리 예산 admission과 oom_score_adj로 늘린다(자문 Q4).
- 동시 쓰기 워커 최대 2, 무거운 로컬 검사 한 묶음, worktree별 target, 실행별 DB/역할/포트.
  독립 검토는 별도 세션이며 코디네이터 자기 검토를 독립 검토로 표시하지 않는다.

## 5. 알려진 결함·위험

- 협업 caret [1,1] 간헐 실패는 #24(awareness 갱신이 native caret을 덮는 제품 결함)로 수정했다. ACL poll이
  한 틱씩 건너뛰던 결함은 #27로 수정했다(철회 지연 2배 → 설정값).
- 협업 room 용량: 서버당 기본 4 room, helper 8개 즉시 거부. 원본 대비 제약이며 수정 진행 중. live room마다
  소유 fence(advisory lock)용 PG 연결을 하나씩 점유하므로 room 수는 PG `max_connections`에 묶인다.
- collab_product는 디버그 helper·병렬 56 스레드에서 짧은 내부 deadline 테스트가 간헐 실패할 수 있다(CI 정상).
  테스트 harness 수정과 용량 작업에서 함께 다룬다.
- 실제 OS IME·특정 기기 입력은 미검증이다. 합성 이벤트 성공을 IME 검증으로 표시하지 않는다.
- 제한기는 프로세스 로컬·직접 socket IP 기준이며 신뢰 프록시·분산 제한은 없다.
- 첨부 중단 업로드·임시 파일·협업 receipt/이벤트/감사 누적의 보존·정리 정책은 원본 대조 후 관련
  slice에서 닫는다. 응답 유실을 실패로 간주해 성공한 저장을 지우지 않는다.
- 원본 TypeScript 설치에서의 데이터 이전은 지원하지 않는다(Rust 스키마 간 업그레이드와 구분).

## 6. 재개

1. `AGENTS.md` → `.agents/environment.md` → 이 문서 → Orca `worker-list --run run_b01d432a9dee` →
   `git worktree list`·각 worktree `git status` → 열린 PR·main CI 순으로 실제 상태를 확인한다.
2. 진행 중 worktree의 미수락 커밋을 보존하고 같은 작업을 중복 배정하지 않는다.
3. 로컬 DB 검사: `scripts/start-test-postgres.sh cargo test --locked --offline --no-fail-fast
   --features db-tests --test <suite>`. 서버 실행 전 `fvoci-migrate` → `fvoci-migrate --grant-app-role <role>`.
4. 설치 확인: `scripts/install-smoke.sh`(RUNNING.md "Container install").
5. 다음 기능은 위 대응표의 미착수·부분 행에서 의존성이 준비된 사용자 흐름을 먼저 고른다.
