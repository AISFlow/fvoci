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
최신 수락 main `1f243d9` (#67까지; 열린 PR은 아래). main CI 성공은
전체 포팅 완료나 협업 용량 수락을 뜻하지 않는다.

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
| #36 | ed5e401 | 문서: #31–35 기록 | CI |
| #37 | 7cd21aa | E2E: 로그아웃이 /login에 도달한 뒤 재로그인(진행 중 logout과의 경합 제거) | 전체 E2E, 자문 검토 |
| #38 | 519eafb | 태스크 담당자·프로젝트 라벨(015)·목록 필터·반복 회차 복사·편집 UI | task_labels5·task64·E2E, 검토+delta(교착 순서 수정) |
| #39 | ae8fc1f | 그룹·프로젝트/문서 멤버 user XOR group(016)·guest 위키 부여·단일 가시성/권한 함수 | group12·collab 전체·E2E, 검토+delta×2 |
| #40 | 005e33c | 프로젝트 마일스톤·태스크 의존성(017)·경합 없는 cycle 검출 | milestones6·task64·E2E, 검토 |
| #41 | 8c23922 | 문서: main ae8fc1f 기준 정정 | CI |
| #42, #44, #51 | 387e60f, e5ff5d2, 441e56f | 테스트 경합 수정(첨부 중단 temp 정리 대기·outbox failure 정리 대기·알림 drain 전 xid 정착) + 복구 snapshot 경계 올림(제품 수정) | 전체 CI, 자문 검토 |
| #43 | 667b133 | 목록 필터 milestone/label을 대상 프로젝트로 제한(원본 동등) | 자문 검토 |
| #45 | 222d3fa | 앱 내 알림(018): PgOnly 소비자·수신자 전달 시점 권한·재생 중복 방지·업그레이드 시 과거 이벤트 제외 | notification3·E2E, 검토+delta×2 |
| #47 | 1791015 | 태스크 댓글 패널·검색형 상위 태스크 선택 | E2E, 검토 |
| #48 | f45bb7b | 전역 검색·댓글 hit·partial 추출 chunk·협업 본문 재색인 축소·검색 E2E | search_index10·search_query4, 검토+delta |
| #50 | 62909be | 그룹 부여 SQL 단일 helper·#40/#47 후속 동등성 | 검토 |
| #52 | 3e79f54 | 프로젝트 홈·프로젝트 문서 API(tree/create/move)·프로젝트 복제·lead 선택 | project38·E2E, 검토+delta×2 |
| #54, #55 | — | 문서 기록, 설치 smoke 프레임 유실 수정(per-socket inbox) | CI, 자문 검토 |
| #49, #56 | 2901d7a, 179a96e | 휴일·ICS 피드, workspace 카드 count·owner 전용 workspace 삭제 | 검토 |
| #46 | c3170b9 | 협업 room 용량: 메모리 예산 admission(기본 30 room, 설정 상한 512), 1013 거부·슬롯 회수, primary admission, fence 상실 시 room 종료, 시작 시 PG 연결 예산 검사 | 64 room release probe(p95 103 ms·writer 손실 0·1011 0·hostile 5/5·대량 재접속·SIGTERM drain), 컨테이너 smoke x64/ARM64, 검토+delta×3 |
| #53 | 4a13b51 | SMTP 메일(lettre, STARTTLS opportunistic)·초대 메일·비밀번호 재설정(020)·digest | mail6·E2E, 검토+delta |
| #57 | 0c85563 | 검색 색인 outbox 배치(범용 deliver_batch)·workspace/rebuild 잠금 취소 안전성·연속 prefix cursor·병합 범위 보존 | search_index·outbox, 검토+delta |
| #59 | baada11 | Web CI: API 계약·schema·unit 공통 검사를 독립 job 1회로(E2E 행은 시나리오만) | 원격 CI 전후 측정, 검토 |
| #62 | d3bf931 | 임시 All-Opus 운영 규칙·TS 데이터 이전 범위 제외 기록 | CI |
| #58 | cccec45 | 첨부 viewer route·프로젝트 문서 댓글·그룹 멘션(전달 시점 재전개) | E2E, 검토+delta |
| #61 | b0d40ea | 단일 in-process 유지보수 실행기(021): 삭제 30일 workspace purge(저장소 먼저)·magic token/processed_events GC·만료 ICS token·digest claim/반환 | background_jobs8·workspace_lifecycle, 검토+delta |
| #60 | 608ec4e | 태스크 activity(022): 변경·반복 회차 기록의 동일 트랜잭션 기록, 변경+댓글 키셋, 그룹 포함 권한, append-only RLS, 공유 댓글 동작 | activity9·E2E 4종, 검토+delta |
| #63, #65 | 7c10edb, adf1aa5 | S3 저장소(rusty-s3, API 프록시): 단일 ObjectStorage 계약(part·complete·range·추출·GC·purge), part 스트리밍·길이 검증, 유지보수 실행기 안의 중단 업로드 GC(공정 cursor), S3 purge(multipart abort 후 객체·DB), 복구 `--verify-storage`, part 동시 수 상한(사용자별 몫)·본문 deadline | s3 15·attachment 23·jobs 10, 검토+delta×4 |
| #66 | 17748ea | 문서 가져오기/내보내기(023): md/pdf/docx/pptx 내보내기(프로젝트 경로 포함), markdown-zip 동기·office/notion 비동기 job(영속 payload·lease/fencing·보상·유지보수 sweep·종료 drain), zip·helper 자원 한도, 이미지에 convert helper 포함 | import/export 15·zip bomb·E2E, 검토+delta×2 |
| #64, #67 | 5a11e7d, 1f243d9 | 문서 기록, collab 테스트의 auth 전 close frame 진단 | CI |

검증 기준: 각 PR의 최종 HEAD에서 원격 Rust/Web/Native/Container install 워크플로가 실제 실행되고
(PG suite는 `--no-fail-fast`), 별도 세션의 Opus 5.5 medium 검토 차단 사항이 해소된 뒤 기대 HEAD로
squash merge했다. 세부 run id·검토 보고서는 각 PR 코멘트에 있다.

진행 중(미수락, 2026-09-26):
- 공유 링크·즐겨찾기·최근(branch `fvoci/rust-share-stars`, migration 024): 독립 검토 ACCEPT(425366c) 후 should-fix
  반영(578c3d6), `/share/:token/pdf`를 #66 helper에 연결하는 중. 공유 정책 설정(instance settings) 미이식.
- 계정 생명주기(branch `fvoci/rust-account-lifecycle`, migration 025): 탈퇴·취소·유예 후 익명화(유지보수 sweep),
  비밀번호·이메일 변경, magic link 로그인, `/me/export`·dashboard·locate. 독립 검토 중. MFA·OIDC·SSO·동의는 미착수.
- S3의 원본 presigned part PUT·302 presigned GET·`S3_PUBLIC_ENDPOINT`·bucket CORS 계약은 미착수(프록시 수락과
  별개).
- 문서 가져오기 잔여: office PDF/DOCX/PPTX/XLSX/ODF, Notion CSV→태스크·자산→첨부·`projectId`, 이벤트 지연 발행.
  의도적 차이: 비동기 실행은 가져온 사용자의 세션이 살아 있어야 하고, 본문 한도는 JSON ≈85 MiB(디코드 64 MiB).

범위 결정(사용자 확인 2026-09-25): 기존 TypeScript FVOCI 배포가 없으므로 TS 데이터 이전(스키마 변환·사용자/세션/토큰
이전·문서 corpus 일괄 이전·dual-write·TS 복귀)은 범위 밖이다. 신규 설치·Rust 스키마 migration·Rust 저장 데이터의
재시작/crash 복원·백업/복구·업그레이드와 제품의 문서 가져오기/내보내기는 범위다.

협업 room 수 구분: 제품 기본값 30, compose 설치 64(PostgreSQL max_connections 150), 검증 64 room×2 사용자 180 s
(단일 WSL2 호스트), 설정 상한 512는 미검증.

## 3. 기능 대응표

상태: 수락(main 반영·검증) / 부분(일부 경로 수락) / 진행 / 미착수. "남은 차이"는 항목별로 진행·미착수·미검증·
후속(수락 범위의 비차단 개선)으로 구분하며, 전체 포팅 완료 전
해결하거나 사용자가 승인한 차이로 기록해야 하는 항목이다.

| 기능 | 원본 근거 | 보존할 불변식 | 상태 | 증거 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·로그인·세션·프로필 | identity/routes.ts, core/auth.ts | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | 부분 | #1, #34, #53 | 수락: 비밀번호 재설정(#53). 미착수: OIDC·MFA·계정 생명주기·설정 기반 비밀번호 정책 |
| 워크스페이스 | domains/workspaces | 현재 역할·철회 경합·RLS·풀 컨텍스트 | 부분 | #4, #39, #56, #61 | 수락: counts·owner 전용 삭제(#56), 30일 purge 실행기(#61), S3 저장소 purge(#63). 미착수: workspace/guest/storage quota |
| 멤버·초대 | invitation.ts, quota.ts, consent.ts | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | 부분 | #21, #53 | 수락: 초대 메일(#53). 미착수: 수락 시 MFA/OIDC, legal consent 428, pending 목록/철회 API, 알림 설정 기본값, 계정 삭제 시 pending 정리, 다른 E2E의 SQL fixture 멤버 |
| 그룹·권한 통합 | policies.ts effectivePermission, project/document_members(user XOR group) | 리소스별 단일 권한 함수 | 수락 | #23, #39, #50 | 후속: collab 프레임당 권한 재조회 축소·collab_delivery의 그룹 join 사본·설정 UI `canManage` DTO |
| 프로젝트 | domains/projects | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | 부분 | #13, #39, #52 | 미착수: 프로젝트 문서 본문(협업)·trash/restore/sort/duplicate route, collection/view 복제 |
| 태스크 | domains/tasks, core/task.ts, workflow.ts | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | 부분 | #13, #19, #26, #38, #40, #47, #60 | 수락: activity feed(#60). 미착수: 저장된 views(캘린더 view 포함) |
| 일정·ICS·휴일 | routes.ts ics/holidays | 일정 의미 | 부분 | #49 | 수락: 휴일·ICS 피드(담당 태스크). 미착수: views 기반 ICS 분기 |
| 위키 문서 | domains/documents, core/document.ts | 현재 문서 권한·트리 잠금 순서 | 부분 | #5, #23, #39 | 수락: 가져오기·내보내기(#66, 잔여는 §2). 진행: 공유 링크. 미착수: 확장 문서 기능, trash 영구 삭제 GC |
| 리비전 | documents/revisions.ts, core/revision.ts, collab applyRestore | 복원은 room actor의 forward system update, durable 후 broadcast | 수락 | #25 | 리비전 routes의 `document_permission` 통합, writer-stale 후 committed_loaded 재설정(후속) |
| 댓글 | comments/routes.ts, core/comment.ts | 문서 XOR 태스크·부모 활성·권한 | 부분 | #28, #47, #58 | 수락: 그룹 멘션·프로젝트 문서 댓글(#58; 그룹 멘션은 원본 snapshot과 달리 전달 시점 재전개). 후속: 이중 DELETE 중복 이벤트·resolve 경합·동시 trash된 태스크 댓글 |
| 협업 | domains/collab, React/Tiptap | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | 부분(opt-in) | #6, #7, #18, #24, #27, #39, #46 | 수락: room 용량(기본 30, 64 검증; §2). 미검증: 실제 OS IME. 미착수: opt-in 해제 조건 |
| 첨부 | domains/attachments, packages/storage | 부모 권한·원본 bytes·원자 완료·취소 | 부분 | #10, #39, #58 | 수락: viewer route(#58), S3 프록시·중단 업로드 GC(#63, #65). 미착수: S3 presigned 계약, 썸네일, 태스크 등 다른 부모, quota·디스크 부족 의미 |
| HWP/HWPX 추출 | 원본 추출 경로, rhwp e8800c8 | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | 부분 | #2, #8, #9, #11, #35 | 미착수: 썸네일 연결. 후속: lease 만료·재시도 결과 게시 경계 |
| 검색·색인·AI | domains/search, packages/search | 검색에서도 인가·철회·색인 복구 | 부분 | #29, #30, #35, #48 | 수락: 워크스페이스·전역 검색, 댓글 hit, outbox 색인, 복구 후 rebuild, 검색 E2E. 미착수: 의미(벡터) 검색, 첨부 viewer route(첨부 hit는 상위 문서로 이동). 후속: 색인 처리량(아래 §5) |
| 알림·outbox·메일·webhook·연동 | domains/notifications, packages/jobs | 커밋 후 전달·중복/재시도 | 부분 | #31, #35, #45 | 수락: 앱 내 알림, 메일·digest(#53, #61). 미착수: webhook·연동, requeue 운영 API/UI |
| 공유·즐겨찾기·최근·태그·컬렉션 | 해당 routes | 공유 링크 권한 | 미착수 | — | 전체 |
| 동의·감사·사용권·관리 | legal, auth.consents, admin.audit, packages/ee | 동의 gate·증거·권한 | 미착수 | 감사 원자 기록만 | 전체 |
| 제품 MCP·CLI·백업·복구 | init.ts, backup.ts, doctor.ts, MCP | 프로토콜·복원 | 부분 | #32, #31, #35 | 수락: 컨테이너 설치 백업·복구(outbox cursor 재기준·검색 rebuild 포함), S3는 스크립트 백업 거부·복구 후 `--verify-storage`(#63). 미착수: 제품 MCP·CLI·doctor |
| 설치·배포 산출물 | infra/app, compose | 비특권 실행·migrate/grant 분리·helper 포함 | 부분 | #17, #20, #29, #32, #35 | 미착수: 운영 TLS/secure cookie 안내, 업그레이드 경로 |
| 추가 DB·플랫폼 | PR999 packages/db (SQLite/libSQL/Turso) | 원본 제공 범위와 목표 구분 | 미착수 | — | PG 우선; 원본 다중 DB를 완료로 간주하지 않음 |
| 프론트엔드 | apps/web, packages/editor | 한국어·접근성·기존 흐름 | 부분 | 각 PR E2E | 미착수: 위 미착수 기능의 UI 전체 |

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
- 협업 room마다 소유 fence(advisory lock)용 PG 연결(풀 밖)을 하나씩 점유한다. 필요 연결 = room 수 + 앱 풀 + reserve 10이며
  시작 시 `max_connections`를 검사한다(#46). 유지보수 job claim이 실행 중 1개를 추가로 쓰며 reserve 안에 든다.
- collab_product는 디버그 helper·병렬 56 스레드에서 짧은 내부 deadline 테스트가 간헐 실패할 수 있다(CI 정상).
  테스트 harness 수정과 용량 작업에서 함께 다룬다.
- 실제 OS IME·특정 기기 입력은 미검증이다. 합성 이벤트 성공을 IME 검증으로 표시하지 않는다.
- 제한기는 프로세스 로컬·직접 socket IP 기준이며 신뢰 프록시·분산 제한은 없다.
- 협업 receipt/이벤트/감사 누적은 원본과 같이 보존 정책이 없다. 첨부 중단 업로드 정리는 #63·#65로 수락했다.
- 전역 보안 헤더(원본 nosecone CSP·Referrer-Policy 등)는 미이식이다. 공유 응답만 개별 CSP·no-referrer를 둔다.
- 가져오기 POST는 요청당 최대 수백 MiB를 버퍼링하며 프로세스 전체 동시 수 상한이 없다(원본도 버퍼링). convert
  helper는 문서마다 node 2회 실행이며 런타임 이미지를 약 390 MB 키운다. 사전 컴파일·dev 의존성 제외가 후속이다.
- collab_product `collab_empty_byte_update_is_rejected`가 #66 CI에서 1회 auth 전 close로 실패했다(로컬 7회
  재현 실패, 재실행 성공). #67로 close code를 남기며 재발 시 원인을 닫는다.
- 컨테이너 AppArmor docker-default 환경(예: GitHub runner)은 helper의 `oom_score_adj=1000` 쓰기를 거부한다. helper는 그대로 시작하고 서버가 1회 경고를 남기며, 이때 cgroup OOM이 서버 대신 helper를 고른다는 보장은 없다(컨테이너 mem_limit·helper별 AS/RSS 한도가 상한). #46
- 검색 색인 소비자는 Meili 단건 쓰기마다 작업 완료를 기다려(측정 1.7–2.6 s) 직렬 처리량이 약 0.5 event/s다. 항상
  수렴하지만 부하 시 색인이 지연된다. 배치/비동기 대기 개선이 후속이다.
- 초대·ICS 토큰이 URL 경로에 있어 `RUST_LOG=debug`/`tower_http=debug`에서 요청 URI 로그로 남을 수 있다(기본 info는
  기록하지 않음). 토큰 경로 마스킹이 후속이다.
- TS 데이터 이전은 사용자 확인으로 범위 밖이다(§2).

## 6. 재개

1. `AGENTS.md` → `.agents/environment.md` → 이 문서 → Orca `worker-list --run run_b01d432a9dee` →
   `git worktree list`·각 worktree `git status` → 열린 PR·main CI 순으로 실제 상태를 확인한다.
2. 진행 중 worktree의 미수락 커밋을 보존하고 같은 작업을 중복 배정하지 않는다.
3. 로컬 DB 검사: `scripts/start-test-postgres.sh cargo test --locked --offline --no-fail-fast
   --features db-tests --test <suite>`. 서버 실행 전 `fvoci-migrate` → `fvoci-migrate --grant-app-role <role>`.
4. 설치 확인: `scripts/install-smoke.sh`(RUNNING.md "Container install").
5. 다음 기능은 위 대응표의 미착수·부분 행에서 의존성이 준비된 사용자 흐름을 먼저 고른다.
