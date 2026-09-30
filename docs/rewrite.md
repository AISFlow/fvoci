# FVOCI 재작성 — 현재 상태와 실행 TODO

이 문서는 기능 대응, #272 통합 후보, 미해결 사항과 다음 수락 행동의 정본이다.
역할·권한·병렬 실행은 [AGENTS.md](../AGENTS.md), 모델·Run 연결·실행 환경은
[환경 기록](../.agents/environment.md), 설치·복구는 [RUNNING.md](../RUNNING.md),
릴리스 절차는 [RELEASING.md](RELEASING.md)를 따른다. 과거 지시는 현재 실행 절차에 두지 않는다.

## 1. 범위와 현재 체크포인트

합의된 Rust 백엔드와 Vue 3 + Nuxt UI + Tiptap 프론트엔드를 보존하고 실제 사용자 URL,
Rust/API·제한된 앱 역할 DB, 저장·복구·권한, 실제 제품 이미지까지 검증한다.
프론트엔드는 **#272 한 후보에 누적**한다. **별도 사용자 승인 전 #272 머지·태그·릴리스·제품 배포 금지**다.
문서 정리는 제품 구현·최종 수락·배포 완료를 뜻하지 않는다.

### 1.1 고정 기준과 관측

관측: **2026-10-01 00:38 KST**. 원격 REST·Git ref, 로컬 Git, 실제 CI job 및 Run task를 대조했다.
문서의 이후 커밋 SHA를 이 표에 재귀적으로 기록하지 않는다. 다음 제출의 SHA는 제출 보고서에 둔다.

| 구분 | 관측한 값 | 의미 |
| --- | --- | --- |
| 대상 초기 기준 | `97a3fe61ede69390b78beaf2de8dd394ad49eed1` | AISFlow/fvoci 초기 tree |
| 원본 main / 조사 기준 | `e95b81a74f175e37d0bbe5b8494481d7a4be2a5f` / PR #999 `393795261322b916e588043cf94feca999175843` | 별도 읽기 전용 원본; 최신 원본을 따라 범위를 자동 확대하지 않음 |
| 원격 main / #272 base | `50d95df1a98c2d88d28f09222c2985fbdb585623` | 기존 main 수락 지점; 이번 Vue 합본은 main 미반영 |
| 원격 #272 HEAD | `f442a9f06c438b51524e13cb7a2043ff5d95566a` | `fvoci/r11-vue-home`, OPEN·Draft, auto-merge 없음; 사용자 요청의 정상 push 완료 |
| 로컬 통합 HEAD | `f442a9f06c438b51524e13cb7a2043ff5d95566a` | 제품 입력은 `589a7db184233f5e69f463f14d914811f96da7da`; 뒤의 세 커밋은 문서 전용 |
| 문서 정리 입력 | f442의 전체 문서 + 이후 코디네이터 미커밋 delta | 되돌림 없이 반영; 단독 작성자 코디네이터, 기존 워커의 문서 WIP 없음 확인 |
| 게시된 버전 | [v0.3.0](https://github.com/AISFlow/fvoci/releases/tag/v0.3.0), `6f64febc487808246596f02922b1826b4fcc939a` | 2026-09-29 12:37:09Z 게시; #272 미포함 |

기존 Run·실행 연결은 환경 기록의 Sol 코디네이터 인수 항목을 따른다. 현재 Run은
`run_496803f4d94f`, 단일 코디네이터는 `gpt-6.1-sol/high`다. 기존 일곱 워커와 WIP를 보존했고,
완료된 worker_done은 결과 회수·자원 확인 후 공식 release 결과를 기록했다. 작업·근거·브랜치는 삭제하지 않았다.
과거 별도 goal thread의 blocked 기록을 새 Run/goal로 reset하거나 active라고 바꾸지 않았다.

### 1.2 실제 검증과 미완료

검사 수는 해당 SHA·환경·범위에만 유효하다. 아래 성공을 합산해 최종 후보 전체 성공으로 표현하지 않는다.

| 검증 SHA / 환경 | 실제 범위·결과 | 현재 판단 |
| --- | --- | --- |
| main `50d95df1` / 원격 | install·Rust·Web·native documents·native collaboration 5개 gate 성공 | 기존 main 근거; #272 수락 근거 아님 |
| PR HEAD f442 / 원격 PR workflow | Rust [36732473149](https://github.com/AISFlow/fvoci/actions/runs/36732473149), Install [36732473096](https://github.com/AISFlow/fvoci/actions/runs/36732473096), Documents [36732473166](https://github.com/AISFlow/fvoci/actions/runs/36732473166), Collab [36732473144](https://github.com/AISFlow/fvoci/actions/runs/36732473144) 성공 | 각 실제 job의 checkout·feature·skip 범위는 연결한 evidence; 로컬 미통합 worker SHA로 확대하지 않음 |
| PR HEAD f442 / [Web 36732473231](https://github.com/AISFlow/fvoci/actions/runs/36732473231) | web-checks·shard 0/1/2/3/4/6/7 성공; web-static·collaboration-flow·shard 5 실패, gate 실패 | 최종 CI 차단. 정적 lint 미완료, archived-writer selectOption timeout, discovery-cache close 실패를 담당자에게 전달 |
| 제품 `589a7db1` / 로컬 pinned Bun | 편집기·i18n 합본 80파일 lint·웹 타입 통과; 인증 범위 lint·59단위·웹 타입 통과 | 각각 독립 검토 후 통합. 이전 인증 `0ced3fa5`의 11 browser와 수정 `3fad750d`의 6 browser를 구분 |
| 디자인 `b99700ab` → `f702` 통합 / Rust·앱 역할 DB·Chromium | 회귀 32건 + 화면·조작 5건, 78 PNG/41상태, 별도 검토 ACCEPT | 필요한 디자인 delta만 최종 후보에서 확인; 물리 touch·새 제목 textarea의 OS IME 검증 아님 |
| 입력·Calendar·Rust close 순서 `62e0d504` / `421e0f4c` / `bc9e05f4` | 고정 범위별 독립 검토 수락, bc9 실제 wiki browser 9/9 | 수정 통합됨. 새 합본 입력·저장·Calendar 회귀와 default-feature 최종 이미지 검증은 남음 |
| 현재 lint 실행 차수 base `95df2170` / 로컬 | 기준 710파일·3,660오류·43경고·fatal 0; f442 전체 format 기준 388파일 실패 | 기준 실패 근거. 워커별 0 진단을 전체 범위 0으로 표시하지 않음 |

상세 source/version·명령·결과·실패·검증 input hash는 §8 evidence에 있다.
취소, 조건부 skip, 미실행, discovery만 실행, 실패 뒤 미실행은 성공과 분리한다.

## 2. 유지하는 제품 결정

- 제품 서버는 Rust stable/Tokio/axum 0.8/Tower/SQLx·PostgreSQL/Serde/tracing이다.
  Yrs 협업과 rhwp·문서 parser는 격리 native child로 실행한다. 이번 차수에는 Yjs/Yrs 교체를 끼워 넣지 않는다.
  차수 뒤 승인된 비교와 채택 판단은 §6에 남긴다.
- PostgreSQL은 현재 원본의 실제 제품 backend다. SQLite/libSQL/Turso의 원본 부분 adapter를
  Rust 제품 지원 완료로 표시하거나 새 backend 구현 범위로 자동 확대하지 않는다.
- 권한은 구체적인 제품 연산·DB에서 재검사한다. 세션·RLS·현재 리소스 권한·원자성·잠금 순서,
  migration checksum과 schema gate를 유지한다. 준비 단계만 소유자 URL·Meili master key를 쓰고
  정상 서버는 제한된 앱 역할과 scoped 검색 키를 가진다. 검색 필터 뒤 PG hydrate가 보안 경계다.
  Meili 부재는 검색 503이고, 키 거부 401/403은 기동 실패다.
- 설치는 사용자 `.env`와 `compose.user.yml`, 앱 컨테이너 시작 절차의 설정 검증·DB/migration/grant·검색 준비다.
  정상 서버는 uid 1000이다. 제품 요청을 JS 서버·JS worker·내장 JS 엔진·외부 변환 서비스로 위임하지 않는다.
  Bun/Node 개발 도구·CodeGraph·TS/PDF 비교 oracle와 브라우저 JS는 제품 서버 runtime과 구분한다.
- TOTP는 `totp-rs`의 계산·검증·base32를 쓰고 URI/recovery/seal/replay는 FVOCI가 소유한다.
  OIDC/JWT는 `openidconnect`와 SSRF 가드 client다. MCP stdio의 기존 계약·16 MiB stdin cap을 유지한다.
  고정 rmcp adapter는 원본 오류·schema·버전/클라이언트 discover 계약과 비용을 검토한 뒤 별도 판단한다.
- 의도적 차이: 프로필 변경 감사, 세션 철회 중 쓰기 차단 강화, 429 정합, 엄격한 ISO 날짜를 유지한다.
  원본에서 project 문서 그룹 route가 항상 404이므로 성공 API를 새로 만들지 않는다.
  Rust 미등록 404와 원본 일부 401/400의 차이는 승인된 표면 차이다.
- 기존 CLI 기능은 Rust migrate/server와 backup/restore 스크립트로 대응한다.
  `outbox-reset`은 skip 진단·이유 기록이며 `--recover-outbox`로 대체되지 않는다.
  원본 all/api/worker/compact/thumbnail/collab 분리 명령은 단일 서버의 내부 연산으로 대응한다.
  개발 seed/mailbox와 Python oracle는 제품 기능으로 세지 않는다.
- 프론트엔드는 pinned Bun·Vue 3·Nuxt UI·Tiptap을 사용한다. 공식 UI 패턴을 얇게 연결하되
  Rust/OpenAPI/cookie 인증·인가·CSP/sanitize·upload·collab persist ACK·같은 Y.Doc/provider/schema를 보존한다.
  UCalendar는 날짜 선택기이며 collection event grid나 새로운 독립 event CRUD가 아니다.
  editor link IME/focus·개인 undo·Escape/Tab·modal·selection, Calendar date/datetime/null/DST/version 충돌을 유지한다.

### 2.1 출처와 제3자 고지

실제 복사·각색 destination별 source/SHA·변경 부분과 전체 라이선스 문구는
[웹 NOTICE](../apps/web/NOTICE.md) 및 해당 package 고지가 정본이다. 의존성·폰트·아이콘 고지도 보존한다.
upstream manifest·lockfile 전체를 복사하거나 Zod 3/4 차이 때문에 제품 schema를 바꾸지 않는다.

| 입력 | 고정 SHA / 라이선스 | 적용 범위·근거 |
| --- | --- | --- |
| Nuxt Dashboard | `57e8a76e85ac382f2dd75946aa450afb1b3e4b0d` / MIT | WorkspaceShell의 dashboard panel/nav 패턴; 실제 각색 경로만 고지 |
| Nuxt Editor | `60886bda1442549b90312ab5097a449eff634fd1` / MIT | 기존 editor toolbar/item slots·menu의 실제 각색 |
| Nuxt Calendar | `11809148a32a40612d1d7ddab8aef5372ad46edf` / MIT | controls/날짜 표시·picker 패턴, 기존 날짜/권한 adapter 유지 |
| anthropics/skills frontend-design | `41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f` / Apache-2.0 | 로컬 원문·LICENSE·출처와 b997 디자인 검토 보존 |

Nuxt 세 MIT LICENSE의 copyright는 `Copyright (c) 2025 Nuxt UI Templates`, 고정 파일 SHA-256은
`e40f408c466e72a3b02eabe846ef35cfab210c14daff782a072dd4903d911f93`다.
production HTTP의 전체 NOTICE·dependency/아이콘 전문 확인은 FINAL-IMAGE 수락에 포함한다.

## 3. 기능 대응 요약

### 3.1 기존 Rust 이관 수락

아래는 고정 원본과 기존 main의 이관 수락 요약이다. 각 PR의 상세 구현·실행·검토는 f442 이전 Git 이력에 있다.
외부 witness·정책이 남은 행도 이미 수락된 Rust 구현을 미구현으로 되돌리지 않는다.
| 기능 | 고정 원본의 경로 | 보존할 불변식 | 기존 수락 근거 | 현재 잔여 분류 |
| --- | --- | --- | --- | --- |
| 설치·로그인·세션·프로필 | `identity/routes.ts, core/auth.ts` | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | #1, #34, #53, #69, #72, #233, #235, #236, #241, #248 | Rust 구현 수락; 인증 정책·외부 witness는 §5 |
| 워크스페이스 | `domains/workspaces` | 현재 역할·철회 경합·RLS·풀 컨텍스트 | #4, #39, #56, #61, #63, #110, #134, #201, #208, #234 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 멤버·초대 | `invitation.ts, quota.ts, consent.ts` | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | #21, #53, #69, #72, #77, #84, #236 | Rust 구현 수락; ACC-1은 §5 |
| 그룹·권한 통합 | `policies.ts effectivePermission, project/document_members(user XOR group)` | 리소스별 단일 권한 함수 | #23, #39, #50 | Rust 구현 수락; 의도적 route 차이는 §2, 성능/DTO는 §6 |
| 프로젝트 | `domains/projects` | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | #13, #39, #52, #76, #78, #80, #83, #161, #231 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 태스크 | `domains/tasks, core/task.ts, workflow.ts` | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | #13, #19, #26, #38, #40, #47, #60, #142, #163, #168, #182, #211, #212, #231, #234, #253, #255 | Rust 구현 수락; 날짜 한계는 §5, Vue 합본은 INPUT-CALENDAR |
| 일정·ICS·휴일 | `routes.ts ics/holidays` | 일정 의미 | #49, #76 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 위키 문서 | `domains/documents, core/document.ts` | 현재 문서 권한·트리 잠금 순서 | #5, #23, #39, #66, #70, #78, #83, #231, #235 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 리비전 | `documents/revisions.ts, core/revision.ts, collab applyRestore` | 복원은 room actor의 forward system update, durable 후 broadcast | #25, #104, #123, #128, #151, #161, #194, #231 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 댓글 | `comments/routes.ts, core/comment.ts` | 문서 XOR 태스크·부모 활성·권한 | #28, #47, #58, #146, #231 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 협업 | `domains/collab, React/Tiptap` | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | #6, #7, #18, #24, #27, #39, #46, #114, #131, #194, #238, #244 | Rust 구현 수락; 입력 합본은 INPUT-CALENDAR, 비용·엔진 비교는 §6 |
| 첨부 | `domains/attachments, packages/storage` | 부모 권한·원본 bytes·원자 완료·취소 | #10, #39, #58, #63, #65, #80, #148, #176, #180, #186, #187, #188, #189, #203, #210, #235, #245, #254 | Rust 구현 수락; #149 외부 검증은 §5, Vue/중립 parser는 §4 |
| HWP/HWPX 추출 | `원본 추출 경로, rhwp e8800c8` | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | #2, #8, #9, #11, #35 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 검색·색인·AI | `domains/search, packages/search` | 검색에서도 인가·철회·색인 복구 | #29, #30, #35, #48, #57, #58, #82, #83, #159, #170, #200, #231, #247 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 알림·outbox·메일·webhook·연동 | `domains/notifications, packages/jobs` | 커밋 후 전달·중복/재시도 | #31, #35, #45, #139, #196, #199, #208, #209, #232, #243 | Rust 구현 수락; 전달·digest 한계는 §5/§6 |
| 공유·즐겨찾기·최근·태그·컬렉션 | `해당 routes` | 공유 링크 권한 | #70, #72, #76, #84, #138, #142, #162, #167, #168, #234, #236 | Rust 구현 수락; Share B는 §5, Calendar 합본은 §4 |
| 동의·감사·사용권·관리 | `legal, auth.consents, admin.audit, packages/ee` | 동의 gate·증거·권한 | #72, #84, #159, #165, #205, #233, #247, #254 | Rust 구현 수락; Vue 흐름의 최종 후보 검증은 §3.2/§4 |
| 제품 MCP·CLI·백업·복구 | `init.ts, backup.ts, doctor.ts, MCP` | 프로토콜·복원 | #32, #31, #35, #63, #81, #84, #90, #109, #202, #204, #207, #220, #228, #232, #233, #246 | Rust 구현 수락; CLI 차이·복구 계약은 §2, 개선은 §6 |
| 설치·배포 산출물 | `infra/app, compose` | 비특권 서버 실행·준비 후 제한 역할 서버·helper 포함 | #17, #20, #29, #32, #35, #169, #175, #177, #181, #190, #192, #202, #206, #207, #208, #223, #238, #240, #246 | 기존 이미지 범위 수락; 새 최종 이미지·설치/복구는 FINAL-IMAGE |
| 추가 DB·플랫폼 | `PR999 packages/db (SQLite/libSQL/Turso)` | 원본 제공 범위와 목표 구분 | — | 원본 PG 제품 경로와 동일; SQLite/libSQL/Turso는 원본 부분 adapter, 신규 제품 backend 미승인 |
| 프론트엔드 | `apps/web, packages/editor` | 한국어·접근성·기존 흐름 | 각 PR E2E, #250, #251, #255 | 기존 React·Gantt tracer 수락; 전체 Vue 후보 수락·main·배포는 별도 (§3.2/§4) |

HWP/HWPX·DOCX·XLSX·PPTX·PDF의 추출 plain text, layout viewer, 편집 사본 저장을 구분한다.
Office desktop reflow·차트·편집 동등성을 주장하지 않는다. HWP lease token 게시 fence(#11),
collab actor panic/rejoin(#114)·helper SIGKILL 복구(#131)는 기존 수락이며 새 최초 구현 TODO가 아니다.

### 3.2 Vue 구현·통합과 최종 수락

모든 아래 기능군의 **#272 최종 후보 수락은 미완료, main 반영·배포도 미완료**다.
기존 main의 Gantt(#255, v0.3.0)·wiki(#262)·IME(#268/#287)·controls(#265)·shell(#267) 수락은 보존한다.
과거 프론트엔드 표의 “수락·남은 차이 없음”은 기존 React/tracer 범위이며 전체 Vue 완료 선언이 아니다.

| 대응 ID | 실제 사용자 흐름 | 구현·통합 상태 | 다음 검증 연결 |
| --- | --- | --- | --- |
| FE-Auth-login/setup/invite/rest | `/login`, `/setup`, `/invite/:token`, reset/magic/confirm/cancel/consent, MFA/OIDC·logout | #269/#270/#271/#278의 후보·수정·전용 React 제거를 #272가 승계 | FLOW-REACT, TEST-FLOW, TEST-SPECIAL; 외부 인증은 §5 |
| FE-Home/Legal | `/`, `/legal/:kind`, `/service-info` | #272의 실제 Vue 진입·공개 약관·운영자 정보 통합 | WORKSPACE, DESIGN-DELTA; 관리자 legal과 구분 |
| FE-WS-home/projects/wiki-list/search/nav | `/w/:slug`, projects/wiki/search, my-tasks/notifications/trash | #274/#279/#283 회수·lifecycle/discovery/search 보완·React 제거 통합 | WORKSPACE, PLANNING, SHELL, TEST-FLOW |
| FE-Wiki-collab/editor/shell/chrome | wiki 본문·컨트롤·댓글/공유/별/리비전/export·공통 셸 | 기존 main 기반 + #284 실제 spec·editor/entity/고지·디자인 delta 통합 | DOCUMENTS, SHELL, INPUT-CALENDAR, DESIGN-DELTA |
| FE-IME/Math-draft/Undo-IME/Dispose | #258/#259/#260, 한글 조합·초안·undo·reconnect/dispose | #258/#287/#264 main; #259 `6441b418`·#260 `ea00d043` 등 수정·검토는 후보에 통합 | 이미 통합된 수정의 합본 회귀; 최초 구현·BLOCK 당시 재배정 금지 |
| FE-Proj-tasks/collections/home/Gantt | 프로젝트 home/tasks/table/board/calendar/Gantt, fields/workflow | #276/#282 연결·누락 settings 보완·React 제거 통합 | PLANNING, TEST-PLANNING, INPUT-CALENDAR |
| FE-Doc-project/task | `/w/:slug/:ref`의 프로젝트 문서·태스크 상세, 본문·단일 room·첨부/활동/시간/Origin | #285/#286 통합, prefix·권한·room lifetime 계약 유지 | DOCUMENTS, PLANNING, INPUT-CALENDAR |
| FE-Attach-view/share/upload | 인증·공개 attachment viewer, upload/resume/download·A/B, 지원 형식별 편집/보호 | #275와 neutral parser/runtime 경로 통합; 현재 lint delta는 별도 worker 후보 | DOCUMENTS, NEUTRAL-ATTACH, TEST-SPECIAL; 실제 S3 F 유지 |
| FE-Share-public/Import-export | `/s/:token`, tree/body·공유 검색·첨부; import/export/trash/restore | #281 denial/recovery 보완·#273/#283 기능 통합 | DOCUMENTS, WORKSPACE, TEST-FLOW |
| FE-Settings-ws/account/Admin | workspace settings/document-tags/templates, `/settings/account`, admin/audit/legal | #273/#277·consent/lifetime/expiry·React 제거 통합 | WORKSPACE, TEST-FLOW, TEST-SPECIAL |
| FE-Compat/Docs-ops | #266 fixture/probe 정리, #263 문서 | 프론트 기능군 완료와 별개; 기존 후보/검토 보존 | §6 별도 후속 |

모든 URL의 직접 진입·reload·back·encoded slug/ref·query/hash·catch-all/foreign 404,
역할/권한·오류·확정 저장/재조회가 수락 대상이다. 페이지 파일·라우트 선언·PR 개수는 수락이 아니다.
제품 React host/boot/router·전용 의존성은 이미 후보에서 제거했고 module graph의 React 0 근거가 있다.
개발 PDF React oracle는 별개다. 새 최종 후보의 graph·실제 cold load·WASM 검증은 FLOW-REACT에 남긴다.
원본 Vue 하위 PR의 고유 코드·검사·수정 지적은 회수했으며, closed/superseded는 merge가 아니다(§8).

## 4. #272 현재 실행 TODO

이 절이 단독 경로 소유권과 다음 행동의 정본이다. 문서 작성자는 코디네이터다.
완료된 준비·회수·최초 구현을 다시 배정하지 않고 새 후보에 필요한 delta만 검증한다.

### 4.1 담당 Task와 고정 입력

기본 base `B` = `95df21702748ed72269a16a6b169330591ff3cd5`.
아래 worktree 이름은 `/home/kinesis/orca/workspaces/fvoci/` 아래다.
보고서 basename은 §8의 영속 evidence 디렉터리에서 찾는다. 정확한 path 목록·허용 범위는 인계 JSON/spec/dispatch를 따른다.

| 담당 별칭 | Task / Dispatch | 단독 범위·worktree | 고정 후보와 현재 상태 |
| --- | --- | --- | --- |
| planning | `task_d14f03dcf97f` / `ctx_05ef98fff945` | Vue projects/tasks/collections/gantt + 지정 5 pages; `f272-lint-planning` | B→`26bf96d2`; 구현 제출·독립 ACCEPT, 미통합 |
| documents | `task_e717f5aa34fb` / `ctx_016d7b64de7c` | Vue documents/editor/attachments/share/**comments** + 지정 4 pages; `f272-lint-documents` | B→`64585711`; 구현 제출·별도 `ctx_c0586e7389bd` ACCEPT, 미통합 |
| workspace | `task_fd205a6b1c96` / `ctx_5ec5ac9abeef` | Vue workspace/wiki/search/notifications/**settings 전부**/legal + 지정 pages, 정확한 lib/oidc.test.ts; `f272-lint-workspace` | B→`20cc8fb8`; 구현 제출, 별도 `ctx_86be9befd80a` ACCEPT, 미통합 |
| shell | `task_5b601ba25779` / `ctx_93073cd9cb9d` | Vue root/router/App/main/components/shell/session/composables/collab; `f272-lint-shell` | `7d57ac05` CHANGES REQUESTED → P2 수정 `d682a435` + tooling joint `10170f84`; 재검토·browser delta 대기 |
| planning-tests | `task_92ee124f0d79` / `ctx_8a5b22149d35` | exact planning E2E·perf ownership JSON; `f272-lint-planning-tests` | B→`39154d12`; 범위 lint 0·74단위·22 browser 제출; 별도 검토 대기 |
| flow-tests | `task_1a578ecd0f99` / `ctx_01c07cf7de7e` | 나머지 지정 52 E2E TS·shared helpers; `f272-lint-flow-tests` | B→`c11bc291`의33 browser 성공; bell spec delta `1f0bbcb6` 제공, joint runtime 대기 |
| special-tests | `task_fa673a8f8bb6` / `ctx_9743f470d8aa` | e2e-pending/native-ime/keycloak/s3; `f272-lint-special-tests` | B→`a6cbaf45`(c57 뒤 archive fixture); pending42 pass/12 OS skip·native1test/4scenario, 추가 runtime 진행 |
| neutral-attachments | `task_a8f91e5a2770` / `ctx_4d3f4f20ee81` | 중립 src/features/attachments/**; `f272-neutral-attachments` | f442→`7eb944fb`; lint·149단위·타입/build·5 browser 제출; 별도 검토 대기 |
| neutral-domain | `task_28e719dfd1fa` / `ctx_4b3bd12e9426` | 나머지 중립 features, sw.js, 지정 개발 scripts, test의 node-api-fetch/mock-event-source만; `f272-neutral-domain` | f442→format `226d7281` 이후 WIP; Request fixture 교정,115 code lint 0·123 format·214 combined 단위 통과; browser lane 배정 |
| tooling | `task_e45b35066f42` / `ctx_d1702c3236dd` | 좁은 선언 project·generator·tests·WEB_LINT; `f272-web-sfc-types` | `3b6e11fb`→`99817830`; 별도 `ctx_fcf60a24d1d2` ACCEPT, 미통합 |

workspace의 제외는 이미 수락한 **src/vue/pages 7개**뿐이고 features/settings 구현 전체를 제외하지 않는다.
neutral와 Vue attachments는 서로 다른 디렉터리다. 공통 manifest/lock/config/API는 명시된 소유 조정 없이 수정하지 않는다.
셸 bell 회귀 spec은 flow-tests가 단독 작성하며 shell 구현자와 표시·오류 계약을 조율한다.
이번 차수의 좁은 호환 예외는 LinkPopover의 IME keyCode229, PendingEditsGuard의 beforeunload.returnValue,
서버 sanitize 후 harden한 ShareBodyView의 v-html, Playwright collabApp의 실제 무의존성 `async ({}, use)`다.
정확한 한 행·사유만 허용했으며 설정 완화가 아니다. 기존 editor의 독립 검토된 예외와 새 delta를 구분해 검토한다.

### 4.2 제공할 결과와 수락 조건

| ID | 제공할 결과 | 담당 Task | 기준 SHA | 다음 행동 | 수락 조건 | 차단 사유 | 근거 위치 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| FRONT-LINT | 전체 관리 범위 ESLint·Prettier | 코디네이터 + 위 소유자 | f442 / B | reviewed delta 누적 후 전체 exact 범위 확인 | typed lint 경고·오류 0, format/type·관련 회귀·독립 검토·최종 CI | 전체 lint/format 미완료 | baseline JSON·format log, worker 보고서 |
| VUE-TYPES | 정확한 App.vue 선언 경계 | tooling + shell | `99817830` | 검토된 4파일 통합, shell joint HEAD lint | 실제 compiler 선언·stale 제거·복수 project fail-closed·기존 editor 44 dts 동일 | 통합 SHA 검사 남음; full web emit의 useCommentActions TS2742를 any shim으로 우회 금지 | web-sfc-types implementation/review 보고서 |
| PLANNING | planning lint·권한 guard delta | planning | `26bf96d2` | corrected product+test 함께 통합 | 새 합본 scoped lint/types/관련 boundary 확인 | 미통합 | planning/report/review, cc8의29 browser와26bf의9를 분리 |
| DOCUMENTS | viewer·comment·share·editor lint | documents | `64585711` | 수락된 fixed delta 통합 | narrow 호환 예외·IME setter·lifetime/sanitize 계약 수락 | 미통합 | documents 보고서: 최종68단위,36 browser; 이전321단위 범위 구분 |
| WORKSPACE | settings·discovery·navigation lint | workspace | `20cc8fb8` | 독립 ACCEPT된 fixed delta 통합 후 필요한 합본 boundary 확인 | 역할/동의/OIDC/link-vs-login/lifetime 유지 | 미통합·합본 검증 남음; combined Request fixture 한계는 UNIT-REQUEST로 분리 | workspace 보고서:50owned/56related단위,13 browser; combined 실패 별도 |
| SHELL-P2 | obsolete refresh·bell 실패 소유 수정 | shell + flow-tests | `7d57ac05`→`10170f84` | active scope/QueryClient denial·visible error 회귀, fixed delta 재검토 | A→B/ABA/dispose 후 stale redirect 없음, current denial 유지; failed write alert/no-nav/다음시도 해제 | 제품 P2 교정·94단위·joint lint 0 보고, 별도 재검토·browser test delta 필요 | shell-review-findings 보고서; 기존86단위/9 browser는 이전 SHA |
| NEUTRAL-ATTACH | 실제 parser/worker boundary lint | neutral-attachments | `7eb944fb` | 형식별 기존 full browser·독립 review | bytes/render/sanitize/권한/저장·worker deadline 유지 | 5 browser 제출, 독립 검토 대기 | neutral 보고서·149단위·7fixture base bytes, 초기 combined 준비 실패 보존 |
| NEUTRAL-DOMAIN | 중립 기능·SW·개발 script lint와 fixture | neutral-domain | f442 / `226d7281` 이후 | Request capture/module-order 재현·owned shim 수정, 관련 단위/검토 | fixture constructor·복원 lifecycle 정합, 제품 API lib 불변 | baseline combined211pass/1parent fail(27nested) 보존; 교정214pass, browser·검토 남음 | neutral-domain spec·combined/isolated logs |
| TEST-PLANNING | 날짜/ACK/권한 E2E lint·실행 | planning-tests | `39154d12` | 제출22 browser의 고정 review·통합 | 원래 matcher/title/API writes·policy 유지, 실제 결과·검토 | 독립 검토 전 | planning-tests report·ownership JSON |
| TEST-FLOW | 나머지 사용자 flow/helper lint·실행 | flow-tests | `c11bc291`→`1f0bbcb6` | 성공한33 browser와 bell delta joint 결과 연결 | 실제 schema narrowing·선행 case 유지, helper 계약 검토, no skip/retry/timeout 완화 | 별도 검토·bell joint runtime·f442 shard5 원인 확인 | flow-tests spec·CI shard5 log |
| TEST-SPECIAL | pending·OS IME·Keycloak·S3 fixture delta | special-tests | `a6cbaf45` | collapsed document options를 사용자 disclosure로 열고 재현, private X/IBus·전용 release target·local MinIO 실행 | archive/read-only/ACK·interception teardown 유지, OS와합성 구분·고정 review | pending42 pass/12skip·native4scenario·S3 transfer1 pass; private TMPDIR/OS smoke 준비 실패·Keycloak build 진행; 외부 S3 env 없음 | special report·CI collaboration log |
| INPUT-CALENDAR | 입력·저장·Calendar 최종 합본 회귀 | 코디네이터, 기존 소유 결과 재사용 | `6441b418`/`ea00d043`/`62e0d504`/`421e0f4c`/`bc9e05f4` + 최종 합본 | 현재 입력/본문/ACK/reload/peer/dates 영향 delta 확인 | 실제 native 입력·rapid back/persist·초안·undo·날짜 충돌 유지, 별도 증거 판독 | final HEAD 미고정; 과거03ca 원인 입증과 현재 회귀 성공은 구분 | 입력·wiki·Calendar evidence (§8) |
| FLOW-REACT | 전체 Vue 흐름·React 제품 경로 제거 확인 | 코디네이터 + 흐름 소유자 | 제품 `589a7db1` + lint 합본 | actual URL·route/permission·module graph·cold load/WASM delta | 합의 URL 전체·catch-all/encoded redirect, 제품 React 부재·개발 oracle 보존 | 최종 합본 검증 남음 | backend closeout·Vue-only graph/browser report |
| DESIGN-DELTA | 대표 화면/한국어/접근성·고지 | 코디네이터 + 기존 디자인 검토 재사용 | `b99700ab` + 최종 합본 | 후속 UI delta의 desktop/narrow/error/readonly/keyboard 확인 | 기존 실제 flow·design source/NOTICE·font/icon/license 보존 | final delta 확인 남음 | design-b99700ab·official-templates evidence |
| FINAL-REVIEW | 누적 고정 source/contract 검토 | 구현과 별도 Sol 컨텍스트 | 최종 합본 미고정 | 각 scope verdict + 통합 delta fixed review | 모든 blocking finding 해소, 읽기 전용 prompt·Git 감시 한계 기록 | shell P2·검토 미완료 | 각 independent report; 자기 검토 불인정 |
| FINAL-IMAGE | 실제 최종 제품 이미지·설치/복구 | 코디네이터, 준비 후 검증 배정 | 최종 합본 미고정 | default-feature Docker·NOTICE·제한 역할·지원 arch 설치/복구 | Node/Bun/Deno/내장 JS 없는 제품에서 실제 경로, source/version/hash/실행 근거 | native debug bundle·과거 이미지로 대체 불가 | 기존 c7 image report와 새 후보 report를 구분 |
| FINAL-CI | 실제 원격 누적 수락 | 코디네이터 | 새 최종 PR HEAD 미고정 | reviewed batch 정상 push → 정확한 head/base/event merge CI 판독 | 필수 gate·실제 검사 수·skip/실패 구분, 보호 조건 충족 | f442 Web 실패; worker 후보 미통합 | CI run/job logs·submission receipt |
| FINAL-REPORT | 수락 근거·남은 한계·승인 대기 | 코디네이터 | 최종 제출 SHA | 완료 범위·checks/verdicts·한계·잔존 자원 보고 | 별도 사용자 승인까지 머지/태그/릴리스/배포 보류 | 승인 미수신 | #272 본문·최종 보고 |
| DOC-STATE | 이 문서 재구성·참조/누락 대조 | 코디네이터 + 별도 Sol 문서 검토 | f442 + 이후 보존 delta | 구조/링크/coverage 확인·fixed doc review | 제품 수정 없이 상태 모순·잔여 누락 해소, 다음 제출 묶음 포함 | 문서 독립 검토 전 | §8 문서 delta·coverage 기록 |

현재 제품 browser 각 실행은 fresh own dist·독립 DB/app role·Meili/search prefix·storage·port 0·report를 사용한다.
프론트 전용 bc9 재사용은 411개 입력·5개 binary hash/feature 확인에 묶인다.
읽기 전용 target `f272-rapid-close/target/rapid-close`에 cargo/generator를 실행하지 않는다.
Keycloak의 SHA-stamped release build와 실제 최종 제품 이미지는 그 debug bundle로 대체하지 않는다.
검증 lane은 실제 자원·종료 결과로 조정하며 문서 정리를 이유로 무거운 제품 검사를 다시 실행하지 않는다.

## 5. 결함·정책·외부 검증 한계

### 5.1 실제 미해결 결함과 좁은 확인

| ID | 상태·다음 확인 | 현재 처리 위치 |
| --- | --- | --- |
| SHELL-P2 | obsolete access query의 stale redirect와 bell write failure 무소유를 fixed7d57/base에서 재현; 최소 수정·unit/browser delta·재검토 중 | §4 shell/flow-tests |
| CI-ARCHIVE | f442의 selectOption visibility timeout; 새 collapsed 문서 options를 열지 않은 fixture가 원인 후보. a6의 실제 disclosure+visible assertion으로 고정 archived case 및 pending42 통과; product byte 동일 | §4 special-tests; 별도 review·합본 수락 남음 |
| CI-DISCOVERY | f442 shard5 discovery-cache의 browser/context closed 실패; close 호출·fixture/lifetime 원인 좁게 확인 | §4 flow-tests; timeout 증가·blind rerun 금지 |
| UNIT-REQUEST | openapi-fetch가 shim 설치 전 Request를 capture하는 combined fixture 실패; isolated 성공과 구분 | §4 neutral-domain |
| INPUT-03CA | 과거 observer 없는 rapid/back 입력에서 마지막 숫자 소실. 이후 실제 허용 ref fixture와 현재 wiki9·OS 입력은 성공했지만 과거 원인/제품 수정의 입증은 없음 | INPUT-CALENDAR; 원래 input/socket/back/persist 조건 유지, 새 engine 비교 선행으로 추가하지 않음 |
| BOOT-NETWORK | 첫 chunk `net::ERR_NETWORK_CHANGED` 빈 화면의 원인은 미확정. #237 netlink/settle는 완화, 앱 chunk 재시도 없음 | 재발 때 요청·net-events 시각 대조; 반박된 가설도 기존 evidence 유지 |
| RECONNECT-HISTORY | 과거 main7cbb의 collab-reconnect afterDestroy 1회 실패, 다음995 통과. #264 dispose 수락과 이 실패의 원인 입증을 구분 | 새 관련 delta에서 기존 회귀 확인; 전체 재감사·최초 구현 아님 |
| DEPENDABOT | Bun lockfileVersion2 updater 미지원 및 docker_compose recreate unknown error | §6 별도 의존성 유지보수; 제품 CI gate 성공과 구분 |

#259 수식 초안·#260 조합 undo·#261 컨트롤/셸의 구현·교정은 이미 후보에 통합됐다.
과거 BLOCK 후보를 현재 구현 상태로 복사하지 않는다. 이슈 종결과 0.4.0 출시 전 수락에는
최종 합본의 해당 입력·첨부·컨트롤 회귀·검토·CI가 남으며, 이슈 open을 미구현 전체 선언으로 쓰지 않는다.

### 5.2 사용자 정책 결정과 승인된 차이

- **ACC-1**: OIDC 초대 계정의 초대 이메일 일치·검증 요구는 미결정이다. 원본도 요구하지 않는다.
  Naver/Kakao의 email_verified 차이 때문에 검증 요구 변경을 되돌린 근거를 보존한다.
- **Share B**: 링크 생성자가 권한을 잃은 뒤 링크 유지 여부는 미결정이다. 생성 시 하위 트리 View 검사(Share F1 A)는 수락됐고
  기존 링크를 재검사하는 정책과 구분한다. **#149 A/B 전송 승인과 다른 정책이다.**
- **team SSO GET start login CSRF**, **SSE 750 ms polling 설계**, IPv6 회전/계정별 login 제한은 정책·설계 결정 항목이다.
  현재 limiter는 프로세스 local/direct socket IP이며 trusted proxy·분산 제한이 없다. proxy 뒤 bucket 공유와 namespace 축출 한계가 있다.
- PATCH dueAt 충돌은 브라우저 Date에 맞춰 millisecond 단위 비교하므로 동일 ms 내 sub-ms 차이는 검출하지 않는다.
  Gantt DST 이동의 dueAt 막대 하루 차이·overlap rail은 기존 한계다. 새 Calendar 날짜 우선·expected versions 수정을 이 한계와 혼동하지 않는다.
- presigned part URL 기본900초·download 기본60초는 철회/mode 전환/삭제 뒤 만료까지 유효하다.
  part bytes 업로드와 게시는 분리되며 게시 전 권한을 다시 검사한다. 일괄 무효화는 S3 key 회전,
  server clock/NTP·trace/HAR 서명 URL 노출·네트워크 오류 메시지 한계를 유지한다.
- 협업 room당 fence PG 연결, 앱 pool와 reserve10·maintenance claim이 필요하다. receipt/event/audit 보존 정책은 원본처럼 없다.
  import POST는 큰 요청을 버퍼링하고 프로세스 동시 admission 한계가 남는다. HWP slot 대기와 실행의 각각 timeout은 최대 약2배가 된다.
- 메일 수락 수신자 목록은 최근64 event의 프로세스 메모리만 있어 재시작/lease 인계 시 재발송 가능하다.
  digest 15분/실패 streak 중단은 다음 날 같은 행에서 반복 종료되어 뒤 수신자를 굶길 수 있다.
- SQLx0.8.6 BEGIN 취소의 열린 pool 연결은 #231 acquire 검사로 막는다. 측정 비용·긴 purge의 xmin/SSE/outbox 지연,
  SSE64 stream poll 비용, Meili chunk task 대기/backlog 처리량과 memory ledger mutex 안 `/proc` 조회 비용은 측정 한계다.
- 설치 보안·복귀 한계는 RUNNING.md가 정본이다. environment 값을 Docker 사용자/컨테이너 root·exec 세션이 읽을 수 있음,
  정상 서버에는 준비 credential 없음, non-dumpable server와 같은 uid helper 파일/검색 키 공유,
  AppArmor OOM backstop 재측정 없음, PG `log_statement>=ddl`의 CREATE ROLE PASSWORD 로그 노출을 보존한다.
  0.1.x→0.2 schema044 downgrade 거부, 0.2→0.3 폐기 FILE 변수/backup 호환 및 미완료 presigned 세션의 복귀 위험을 생략하지 않는다.

### 5.3 외부 환경 검증과 지원 범위

| 범위 | 기수락·실행 근거 | 남은 검증 / 종료 조건 |
| --- | --- | --- |
| #149 첨부 전송 A/B | **A/B 모두 승인**, #254 Rust 구현 수락; local MinIO+PG24 및 Chromium cross-origin witness | 실제 AWS S3·다른 S3 호환·CDN/proxy·Firefox/Safari·clock skew. 실제 증거 또는 사용자의 한계 승인까지 F; 정책 선택 대기로 되돌리지 않음 |
| OIDC/MFA | local Keycloak26.7.4 로그인8·실패7·wrongsecret1·SSO1 기존 witness, Rust test entitlement; QR 구조 수락 | 실제 외부 IdP·HTTPS/proxy·container·다른 브라우저·인증 앱 실제 scan. 새 Vue Keycloak scope는 TEST-SPECIAL에서 확인 |
| workspace SSO | workspace callback/mix-up/개인 workspace 거부 등 Rust 수락 | 게시 license trust keys가 비어 있어 enterprise workspaceSso를 켤 수 없음. local test entitlement 성공을 게시 제품 활성 지원으로 표시하지 않음 |
| OS IME·touch·브라우저 | Linux X11 IBus hangul2벌식·Chromium/XTest 실제 witness; multirange40f 선택교체/ACK/reload/undo 범위 수락 | Windows/macOS/mobile/특정기기·물리 keyboard/touch, FF/WebKit은 witness 밖. 합성/CDP 성공을 OS IME 성공으로 표시하지 않음 |
| Web Push/메일/GitHub | Chrome153·Linux·FCM1회 subscription/send/SW receive/logout witness | 다른 browser/provider, 실제 SMTP/GitHub App, settings licensedSSO/clipboard/multipage events의 미실행 범위 유지 |
| 이미지·설치·복구 | 기존 amd64/local, ARM local 고정쌍, local S3 image upgrade/versioned rollback witness | 최종 #272 후보·최신게시 image·ARM S3·게시0.2→0.3 pair·자동upgrade, Mac Docker Desktop/rootless/Podman 범위 구분 |

외부 검증이 남았다는 이유로 수락된 Rust 연산을 미구현으로 되돌리지 않는다.
기존 수락의 비차단 platform 한계를 새 필수 선행으로 추가하지 않으며, 원래 F 항목은 근거 없이 지원 제외하지 않는다.

## 6. #272 이후 후속

이번 PR의 새 필수 완료 조건으로 끼워 넣지 않는다. 기존 승인·고유 WIP·미실행 위험을 보존하고
선행이 준비되면 별도 단독 소유권과 고정 base/head로 이어간다.

| ID | 제공할 결과 / 보존한 입력 | 다음 행동·수락 조건 | 차단·근거 |
| --- | --- | --- | --- |
| BOUNDARY-ALIGN | 승인된 경계 정렬·Gantt pixel layout 계약 정리 | 제품 caller/인가/설치 경계를 맞추고 필요한 Rust/API/DB delta 검토 | Gantt tracked19·identity tracked34의 기존 WIP와 recovery mapping; 새 작성자 미배정, 미검증 WIP |
| #280 COLLAB-COMPARE | 현행 협업 비용·복구·schema 비교, 별도 문서 `b4d5bf25` | 고정 차수 #265/#267/#269/#270/#271 + #287 후 실제 비용·복구·schema 비교; 필요시 제품과 분리한 PM step authority 실험 | 문서 독립 ACCEPT/기존 CI와 실제 비교 미실행을 구분. Yrs 유지 결론 선결정 금지, #272 추가페이지를 새 선행으로 계속 추가하지 않음 |
| THUMBNAIL-REVIEW | 승인된 썸네일 검토 | 원본 범위·현재 product boundary/격리·비용 확인 뒤 결과 제출 | 기존 HWP 썸네일 원본 부재와 별개; 검토 승인을 기본 엔진/데이터 이전 또는 신규 구현 승인으로 확대하지 않음 |
| #266 FE-Compat | `1c0a5a85` probe 제거·golden fixture 보존/이동 | 별도 최신 base 통합·고정 검토/CI 수락; hocus-wire regen의 실제 common resolution 확인 | 원본320 fixture identity·134selector 등 기존 검토 재사용, #272 자동 흡수/삭제/재생성 금지 |
| #263 FE-Docs-ops | 기존 문서 `d556a7eb` 후보·검토 | 이미 #272 ancestry에 보존된 기록을 재구현하지 않고 별도 PR/TODO 처리 상태 확인 | 독립 문서 ACCEPT를 제품 완료로 확대 금지; 이번 정리는 #272 다음 제출 묶음 |
| RECOVERED-WIP | digest `a83c38a3`, import `8f418b2f`, search/streams `aa3cd449`, collab structure `9070bbfd`, deps `11b203b0` | 실제 diff·계약·검증 후 기능별 수락 | apply-check만 통과, 제품 검증 미실행. 기존 patch/bundle 보존; Vue 연결 선행 아님 |
| OPS-DATA-PERF | mail recipient 영속화, digest rotation, import admission, collab/db C11/C12, outbox C10/C12, attachment C9–C12, search SA7/SA9–12, identity HI04/08–11, ops CO10–12 | 필요 scope에서 DB/권한·복구·부하·독립 검토, migration은 별도 소유 조정 | 기존 round3 finding·WIP/정책 유지; sqlx0.9/acquire 제거·try_acquire/try_begin 금지, SSE64/xmin/Meili/room 비용·AppArmor·perf/seccomp 문구 포함 |
| DEPENDENCIES | 아래 별도 PR의 실제 소비자/라이선스/설치 영향 | 최신 base·owned manifest/lock·실제 검사·고정 독립 검토/CI | #272 자동흡수/merge 아님; payment/runner/permission 변경 없음 |

의존성 후보: #191 `ae034156`(provider/Yjs/XLSX/KaTeX/Vite; live4.7 fixture delta 검토 수락, 새 후보 영향 확인),
#156 `763f692e`(QR2.0.4·full MIT notice; 실제 MFA/served license 확인), #160 `7546b780`(collab engine base64),
#158 `98886c09`(root base64), #157 `ece789f0`(thiserror), #155 `87185d93`(PG/Meili),
#153 `b00c059f`(Debian image), #152 `126b1263`(checkout6workflow).
각 과거 실패·skip·독립 verdict는 open-pr takeover raw evidence에 남아 있다. 오래된 check 집계를 현재 성공으로 쓰지 않는다.

엔진 채택·사용자 데이터 이전은 비교 뒤 별도 판단이다. TS 데이터 이전은 기존 사용자 확인으로 현재 범위 밖이다.
HWP 썸네일·MCP HTTP transport·requeue 운영 API/UI·license issuer trust 등 원본 미제공 범위는
새 요청 없이 제품 구현 범위로 만들지 않는다. Prometheus2단계·restore.env quoting·DTO/권한 join 정리,
flushDelay50/100·DocumentView 구독 분리·대규모 room 재설계 등 비차단 개선은 기존 근거에 연결한다.

## 7. 재개와 최종 수락

현재 역할·권한은 AGENTS, 실제 모델/Run 연결은 환경 기록을 읽는다. 그 뒤 이 문서의 §1 체크포인트,
§4 소유권/TODO와 실제 git status/worktree, Run task/worker·미처리 질문·CI를 대조한다.
기존 작업·검토·입력 근거를 재사용하고 재인수·중복 배정·전수 감사·Run reset을 하지 않는다.
Orca 작업은 설치 버전의 live guide를 따른다. 기존 Run을 사용하고 다른 terminal을 사칭하지 않는다.

다음 실행 순서: 미해결 worker 질문/결함 → fixed 독립 verdict → 검토된 delta 로컬 통합 →
필요한 합본 boundary 검사 → 최종 이미지/실제 runtime·누적 검토·CI → 최종 보고와 **사용자 승인 대기**.
독립 검토와 CI는 고정 후보에서 병렬 가능하지만 둘 다 수락해야 한다.
문서 전용 변경은 기존 CI 선택 정책대로 판단하고, 문서 때문에 무거운 제품 검사를 추가하지 않는다.
검사 선택·자원 격리는 fast-verify/handoff 스킬과 기존 검증 명령을 따른다.

최종 수락에는 기능/UI 연결, 보안·데이터·복구, 합의된 DB·platform/산출물, 현재 역할의 독립 검토와
실제 CI가 필요하다. 개별 worker/PR 완료는 전체 종료가 아니다. 미구현·미연결·부분 검증·원본 미제공을 구분한다.
미해결 정책/F를 문서 압축 때문에 해결·제외로 선언하지 않는다.
세션 한계에서는 검증 SHA·미수락 diff·활성 소유권·실패·다음 명령·CI·잔존 자원을 기존 evidence에 남긴다.

## 8. 과거 기록·근거 위치

정리 전 전체 문서: [고정 f442의 rewrite.md](https://github.com/AISFlow/fvoci/blob/f442a9f06c438b51524e13cb7a2043ff5d95566a/docs/rewrite.md).
전체 PR별 일지·옛 HEAD·실패·반박된 가설·dispatch·수량/모델 지시 이력은
`git log -p -- docs/rewrite.md`와 원래 커밋/보고서로 추적한다. 새 대형 archive 문서를 만들지 않는다.

영속 evidence root `E` = `/home/kinesis/orca/fvoci-evidence/recovery-20260930/takeover-evidence/`.
현재 코디네이터의 문서 정리·WIP 보존·새 scope 보고서는 `E/sol-coordinator-docs-20261001/`에
basename과 SHA256SUMS로 회수했다. `/tmp`가 유일한 정본인 자료는 이 사본으로 재개한다.
새 결과는 같은 기존 evidence 위치에 계속 회수하며 위 체크포인트 이후 결과는 다음 제출 보고서에서 구분한다.

| 근거 | 추적 위치·의미 |
| --- | --- |
| 원본 고정 계약·기능표 | f442 §1/§3, 기존 source-contract 조사 보고서; 이 문서21개 기존 행을 §3.1에 대응 |
| main 수락·release | `E/main-50d95df1-gates.json`, f442의 PR merge 표·§6.1 release run/digest; v0.3.0 image index `sha256:bd8d2e45deac97849a7c079a9b856f59a1882735148b3deea1834a4fd50580f9` |
| 기존 회수·미커밋 WIP·정리 | `recovery-20260930/result.json`, 원래 patch/bundle/runtime-map·cleanup receipts; branch/HEAD/WIP 보존 |
| 원본 Vue PR 회수·종결 | `E/source-pr-superseded-48f6246c.json`, `open-pr-takeover-raw/`, source-pr-closure-audit 보고서; #269/270/271/275/276/278/282/285/286 및 #273/274/277/279/281/283/284 승계, closed는 미병합 |
| Rust 제품 경계·React 제거 | backend-closeout·Vue-only graph·editor/viewer retirement·c7 image/install 보고서; 과거 증거를 final 후보 성공으로 복사하지 않음 |
| #259/#260/Calendar/native 입력 | math259·composition260의 최초 BLOCK와 교정 review, `E/ime-multirange-40f090d1/`, wiki21ff/bc9·Calendar62e·caret421·native provenance; OS/CDP/합성 구분 |
| 공식 template/디자인·고지 | `/home/kinesis/orca/fvoci-evidence/front272-official-templates/`, `E/design-b99700ab/`, 원래 source LICENSE·NOTICE·served artifact 확인 |
| editor/auth 이전 lint 수락 | `E/auth-and-editor-589a7db1/`, lib/build/settings/lint-tooling fixed reports·integration ancestry |
| 기존 Run/현재 Task·범위 | sol coordinator handoff TXT/JSON, worker-start spec/dispatch/ownership JSON, 현재 task snapshot·Run receipt; 모델 attestation은 환경 기록 |
| 최신 구현·독립 review | `f272-lint-{planning,documents,workspace,shell}-sol61.txt`, 해당 review 보고서·shell-review-findings, web-sfc-types implementation/review, neutral/E2E 보고서; 모두 위 영속 사본 basename |
| f442 실제 실패·원격 결과 | `pr272.json`, `ci-f442.json`, `web-f442-jobs.json`, `f272-ci-f442-{static,collaboration,shard5}.log`; 취소/skip·미실행 구분 |
| 정리 직전 미커밋 delta | `rewrite-post-f442-uncommitted.patch`, before SHA256, 문서 coverage·검토 보고서; f442 이후 작성자 변경 보존 |

과거 기능 행·FE ID·잔여의 대응은 §3/§4(현재), §5(정책/한계), §6(후속), 위 Git/evidence(기수락/중복/역사)다.
i18n `1172a73a`는 #165 `7f3e2477`과 제품 코드 동일로 기종결이며 재구현하지 않는다.
문서 정리 결과의 독립 verdict·정리 전후 분량·제출 SHA는 다음 제출 보고서에 기록한다.
