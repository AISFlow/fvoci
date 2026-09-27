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
최초 기준 main(역사적 anchor)은 `6ba876503e2a8340c0c21f8f110d2ec14f67f8b3` (#141, 2026-09-27, main 5개 workflow 성공)이다.
최신 수락 제품 main은 아래 `3919a326f384b2f3168eb495f539a9556b536f73` 표(2026-09-28 KST)이며, 그 사이 anchor는 당시 관측으로 보존한다. 열린 제품·CI PR은 아래에
별도로 표시한다. 개별 PR이나 main CI 성공은 전체 포팅 완료를 뜻하지 않는다.

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
| #68 | c36c810 | 문서 기록 | CI |
| #71 | 6a04e12 | CI: PostgreSQL suite를 아키텍처별 2 shard로(x64 14분 → 9.5분+5.1분, timeout 변경 없음) | 원격 CI 측정, 검토 |
| #70 | 4b13a1f | 즐겨찾기·최근·공유 링크(024): 해시 토큰·SELECT 전용 definer 조회, 매 요청 만료·철회·root/trash·subtree 재검사, sanitize HTML·CSP·no-referrer, 공유 PDF(공개 전용 fail-fast helper pool) | share9·E2E, 검토+delta×2 |
| #69 | 4afb18d | 계정 생명주기(025): 탈퇴·취소·유예 후 익명화(유지보수 sweep), 비밀번호·이메일 변경, magic link, `/me/export`(streaming ZIP)·dashboard·locate, 로그인-탈퇴 경합 차단, dev profile argon2 최적화 | account12·E2E, 검토+delta |
| #72 | 4c46252 | 관리(026): audit·system·users·workspaces·instance admin(마지막 관리자 보호), typed instance settings와 공개 projection, 법률 문서·동의·428 consent gate(/collab 포함), branding asset, 공유 정책 연결 | admin11·E2E, 검토+delta |
| #73, #75 | 96b0125, 8341c32 | 문서 기록, 알림 E2E가 전달 완료 후 페이지를 여는 순서 수정 | CI |
| #74 | 265d7d5 | webhook(봉인 secret·서명·SSRF 차단·독립 송신 task)·GitHub App(세션 결속 설치 state·issue 동기화)·AI 동작(원본과 같은 로컬 휴리스틱) (027) | integrations15·E2E, 검토 BLOCK→delta ACCEPT |
| #76 | 0cf6851 | 문서 태그·컬렉션(typed 필드·값 CAS·view-query 단일 컴파일러·calendar/board)·collection/project 저장 view (028); 비슈퍼 owner 업그레이드 backfill | collections7(업그레이드 포함)·E2E, 검토 BLOCK→delta×2 |
| #77 | 56209c7 | TOTP MFA(단일 세션 발급 게이트·DB 기반 시도 제한)·OIDC 로그인/연결·workspace SSO·JIT (029) | identity19·E2E, 검토+delta |
| #78 | 917b4e0 | 프로젝트 문서 협업(단일 `lock_collab_document_access`, 프로젝트 FOR SHARE 잠금)·프로젝트/문서 휴지통·복원·정렬·archive·30일 purge(저장소 먼저) | collab 71/30/20·project_lifecycle4·E2E project-trash, 검토+should-fix 반영 |
| #79 | 5a739c6 | 문서 기록 #73–#77 | CI |
| #80 | a69e623 | 첨부 완성(030): 태스크·프로젝트 문서 부모, DELETE(uploader edit/그 외 manage)+정리 journal, 저장 quota(402, 권한 후 quota), 이미지 preview(rlimit·env_clear child) | attachment23·parents13·preview6·S3 16·E2E, 검토 |
| #83 | 28a86f1 | 문서 API: body GET/PUT(md·json)·block patch(tail 선조건 3회)·children·backlinks(읽기 시 파생)·duplicate·flat routes·PAT 검색 scope; 외부 쓰기는 room actor `ReplaceFromUpdate` 경유 | document_api8·document19·collab 71, 검토 |
| #84 | 90a3df0 | 관리자 사용자 삭제 예약/취소(032)·공유 대화상자 정책·`/s/:token` head meta·MFA QR(SVG)·branding `--verify-storage` | admin16·share10·identity19·E2E, 검토 |
| #82 | 926f01d | 의미 검색(031): 첨부 chunk embedding(extract job 안 embed pass)·workspace hybrid 모드·PAT parent-kind 축소 | search_semantic·search_query, 검토+delta |
| #86 | 069cc4b | 스키마 게이트가 max(version) 대신 전체 migration 집합을 요구(순서 뒤바뀐 031/033/034 대응) | db_integration gate 5, 검토 |
| #87 | 873e440 | 역할 재배정(Fable 코디네이터/검토, Opus 구현, Grok 조사)·Rust 런타임 규칙·CodeGraph 기록·문서 갱신 | CI |
| #89 | 84b0481 | install CI 경로 필터에 convert helper·`.codegraph` 제외 | CI |
| #85 | a6e31dd | 가져오기 잔여(033): office 7종을 같은 바이너리의 격리 child(`--internal-office-extract`)로, Notion CSV 태스크·자산, 지연 import 이벤트; harness stderr 경합 수정 | import 15·formats 8·extract 14·native x64/ARM64, 검토+delta×2 |
| #92 | 1bd7fb8 | collab join 1011 flake: 삼켜진 join 오류 로깅, db-tests 전용 estimate-fail hook을 문서 키로, 공유 harness의 process-wide engine child cap 슬롯 게이트 | collab_lifecycle 20·projection 19·product, 검토+should-fix 반영 |
| #91, #93 | b205058, 78d20f5 | e2e 경합: admin erase 확인 dialog unmount 대기, collections board group-by 정확 label | 재실행 5/5, 검토 |
| #81 | f89e9ea | 전역 보안 헤더(nosecone 동등 CSP·Referrer-Policy 등, 공유 응답 개별 헤더 유지)·제품 MCP 서버(stdio JSON-RPC, PAT)·`fvoci-migrate --doctor/--init-env` | mcp·doctor·share CSP, 검토 |
| #90 | d0942f1 | 인증·복구 차단 수정(035): identity link `issuer`(NULL 허용, write-once definer backfill, 다른 issuer 같은 sub 거부), link 저장 tx의 `recheck_session`, `ENCRYPTION_KEYS` fingerprint+`--verify-secrets` 복구 probe, `--verify-storage` preview 포함, document-extract child `env_clear` | identity24·admin17·db75·backup-restore-smoke, 검토+should-fix 반영 |
| #88 | fd0b01b | 태스크 API(034): time entries·clone·backlinks·purge·flat `/tasks/:id`·parents·workspace 태스크/상태 목록·workflow status CRUD·My Tasks/워크플로 설정 UI | task_ops15·task64·formats8·E2E, 검토+delta×2 |
| #94 | e1319b8 | 스킬: 표준 구현 재사용·Rust 실행 경계 절차, handoff/fast-verify 보강(ChatGPT 준비·사용자 게시) | Fable 검토+should-fix 반영, CI |
| #95 | d4ad152 | 문서 기록 #81–#94 | CI |
| #96 | 225e592 | Markdown→Tiptap Rust 파서(markdown-rs 1.0.0 vendoring, EditMap 2차 복잡도 패치)를 같은 바이너리 `--internal-markdown` child(rlimit·30 s watchdog·동시 2·env_clear)로; body PUT/GET md·가져오기·법률 md→HTML·AI md를 Node에서 분리; oracle corpus 53/53 일치 | markdown_process6·vendor54·document_api·import 15/8·admin16, 검토 BLOCK(vendor target 추적)→수정 delta ACCEPT |
| #97 | 07b75d3 | OIDC id_token 검증·요청 구성을 `openidconnect` 4.0.1로(손수 작성 JWS 검증기 442줄 삭제, SSRF 가드 fetch.rs가 유일한 egress, RS256/ES256만, 64 KiB 토큰 응답 캡, RFC 3339 timestamp 허용) | identity26(부정 벡터 9종)·lib, 보안 검토+delta ACCEPT |
| #98 | 89498c4 | TOTP 코드 계산·검증·base32를 totp-rs 6.0.0로 부분 재사용(URI·recovery·seal·claim_step replay 유지) | totp6·identity24, 검토 |
| #99 | 8e57d18 | Tiptap→Yjs seed를 collab-engine child의 stateless op으로(Yrs, tree 동등성 68/68), body PUT·duplicate·가져오기 Node 분리, seed 전용 child pool(2)·재시도 가능한 import Unavailable·스키마 drift CI 검사 | seed_compat4·document_api11·import 16/8·collab_product72, 검토+delta ACCEPT |
| #101, #103, #106 | bff5cf5, 2e3bbd4, 25a6a0f | 공통 export 모델·격리 Rust child로 DOCX/PDF/PPTX/Markdown·공개 공유 PDF 전환, ConvertClient 제거, 실제 변환 doctor 및 migrate의 server 경로 선택 | 관련 회귀·원격 CI·독립 검토; LibreOffice/Poppler 대표 소비자 검증(전체 MS Office 호환성 증거 아님) |
| #102, #108 | 357ffc6, 737cab5 | 실제 Microsoft issuer/subject 영속화·선택 키 issuer 범위·GUID tid·RSA 실제 비트 하한, 출처 불명 기존 identity의 추정 재매핑 거부 | 부정 벡터·DB identity 회귀·원격 CI·독립 검토 |
| #107 | 11dc53b | 서버 이미지의 Node 제거; 실제 Rust 변환·doctor·가져오기·편집·공유·재시작 연결 | 설치·복구 x64/ARM64 및 관련 원격 CI·별도 sol 검토 |
| #109 | 14fb7d3 | 기존 fvoci-migrate에 백업 manifest·복원 preflight 통합, 공통 Rust Keyring 검증, 운영 Python 호출 대체·파괴적 작업 전 설정/이미지 검증 | 키 누락·오류 거부/rotation·실제 암호문/저장 객체 복구, x64/ARM64 CI·별도 sol 검토 |
| #105, #111 | 07ce70a, 0eeb107 | 현재 모델 역할·실행 설정 갱신(과거 이력 보존) | 별도 검토·원격 CI |
| #113 | cdd4d47 | 일반 Web E2E 28개 그룹 잡을 8개 shard로 통합, shard당 준비·빌드 1회와 그룹별 상태 격리·신규 spec 자동 발견 | 8개 실제 로그에서 29 spec·51 테스트·빌드 각 1회, 독립 검토·CI |
| #110 | 30c5200 | 원본 사용권 정책·좌석/저장 quota·audit/branding/SSO gate, 복구의 기존 branding 객체 검증 보존 | 별도 정책·복구 delta 검토, 통합 HEAD의 26 checks; 컴파일된 issuer trust는 원본처럼 비어 있어 발급 토큰 운영 활성화는 미검증 |
| #104 | ebca941 | 태스크 본문 협업·리비전 기반과 migration 037, 기존 문서/태스크 인가·철회 보존 | 기존 독립 검토와 별도 Grok 통합 delta 검토; HEAD 8ab3dc7f의 26 checks, task 협업 PG 7개·실제 task-body 브라우저 실행; main 후속 CI는 별도 진행 |
| #112 | fee40552 | 가져오기 보상 실패 시 복구 참조·기존 저장 객체 보존 | 가져오기 18개, 별도 검토·26 checks |
| #114, #115 | 0e7a0dc1, aa1a16cd | 중복 Rust 호출 제거·누적 PR diff 기반 선택과 실제 결과 gate | 협업 10 suite·176개 양쪽 arch, 선택 규칙 회귀·별도 검토·원격 CI |
| #116 | be605976 | 수락 기록·사용자 승인 Grok 독립 검토 대체 경로 | 별도 검토·36 checks |
| #117 | accf7b8e | 문서와 태스크 origin API·UI, 현재 인가된 연결 목록 | 제품 회귀·별도 검토·원격 CI |
| #118 | 36ce72f7 | 문서·태스크 템플릿, migration 039 | PG 4개·실제 브라우저 적용 1개, 별도 검토·36 checks |
| #119, #120 | 74ec8fe7, ec5694fc | 공개 운영자 정보 UI·검증된 연락처의 security.txt | 운영자 브라우저 4개·URI 회귀, 별도 검토·각 36 checks |
| #121 | 14669630 | 태스크 archive 전 협업 본문 영속화, 취소 검사 소유권 경계 고정 | archive 브라우저 3개·기존 body 흐름, 별도 검토·36 checks |
| #122 | 2fddf5e0 | Vite 번들 기준 브라우저 오픈소스 고지·제품 링크·배포 입력 | 실제 산출물·브라우저, 별도 검토·최신 5개 workflow 모두 성공; 이전 취소 실행과 구분 |
| #123 | f1d85e9e | 문서·태스크의 committed 본문을 session 종료 시 리비전으로 저장, 자동 리비전의 수동 승격 | PG·협업·브라우저 회귀, 별도 검토·원격 CI |
| #124 | d7270ecf | 인증된 Rust unfurl·허용 embed를 문서/태스크 편집기에 연결 | 별도 검토·원격 CI; UI 브라우저 검사의 API mock과 실제 외부 사이트 검증은 구분 |
| #125 | fe47d9e4 | Rust DB suite 등록 누락·실행 억제 탐지 | 선택 규칙 84개·별도 delta 검토·원격 CI |
| #130 | f159fb55 | Rust project layout과 연결된 Gantt 조회·편집 UI | 관련 API·브라우저 회귀·별도 검토·36 checks |
| #129 | ba6e3e1c | 협업 idle eviction 검사의 타이머 소유권을 명시적 제어로 고정 | x64/ARM64 협업 실행·별도 검토·36 checks |
| #127 | 889118c0 | 에이전트별 worktree CodeGraph 동기화·호출 경로 대조 지침 | 별도 검토·동일 트리의 전체 CI 근거, 현재 base의 선택 계획 재확인 |
| #128 | 6ce8f52d | 문서·태스크 scheduled 리비전과 bounded 자동 보존 정리, 수동 리비전 보존 | background_jobs 25개(x64/ARM64, 승격·GC 경합 포함)·별도 검토·36 checks |
| #131 | 8b75d7f9 | 살아 있는 협업 helper의 SIGKILL 이후 새 컨텍스트에서 복구 확인 | 실제 helper·브라우저 회귀, 별도 검토·원격 CI |
| #132, #137 | 3478aef0, 1b4a3235 | 태스크·프로젝트·접근 철회 SSE와 연결 준비 이후 이벤트를 생성하는 회귀 | PG·실제 UI·x64/ARM64 검사, 별도 검토·각 36 checks |
| #134 | bf3ad9c4 | workspace ZIP 내보내기, 항목 전달 경계의 현재 인가·취소·한도 | workspace_lifecycle 19개·철회 경합·브라우저, 별도 검토·36 checks |
| #136 | bbaf8920 | AGENTS.md·.agents/environment.md의 명시적 문서 분류, 혼합 변경·실패 시 검사 유지 | 선택기 103개·별도 검토·전체 36 checks; #140에서 실제 제품 잡 제외 확인 |
| #138 | a2df144a | 공개 공유 HEAD의 보안 헤더·빈 본문, Content-Length 생략 | static_api 10개(실제 HTTP 포함)·공유 PG 11개·별도 검토·36 checks |
| #139 | 546b1533 | Rust Web Push·migration 040, 발송 직전 현재 권한 확인·로그아웃 시 해당 브라우저 연결 해제 | x64/ARM64 push 9개씩·36 checks·별도 검토; 최종 합성 트리에서 Clippy·push 9/공유 11/static 10 추가 통과. 실제 외부 푸시 서비스는 미검증 |
| #135, #140 | 081cafef, 50268dca | 현재 모델 역할과 프로젝트 전체 쓰기 상한 3개를 운영 기록에 반영 | 별도 검토; #140은 5개 계획·5개 gate 성공, 제품 잡은 not applicable로 실제 제외 |
| #141 | 6ba87650 | 수락 기록·잔여 검증 정리, 기존 OIDC 초대 흐름 수락 표기 | 별도 검토·5개 gate 성공; merge 후 main 5개 workflow 성공 |

기준 main 이후 수락 PR(2026-09-27, merge는 원격 대조). 각 PR은 고정 HEAD에서 36개 check(5개 plan·5개 gate 포함) 성공과
별도 Opus 5.5 medium 독립 검토 ACCEPT 후 머지했다. PR 수락과 merge 후 main push workflow는 구분하며, main 열은
14:55 UTC 관측이다(대기는 통과가 아니다).

| PR | 고정 HEAD | merge | 범위 | main push workflow |
| --- | --- | --- | --- | --- |
| [#142](https://github.com/AISFlow/fvoci/pull/142) | `830851f2b20a162d17291c929e529d5316628905` | `ef7f8977d55669e6b4c92a825c0a711783ab19d2` | 컬렉션 board 그룹별 paging·drag-and-drop 저장 | 5개 성공 |
| [#146](https://github.com/AISFlow/fvoci/pull/146) | `fa3153b88945588e7670644660ee65c46f6d1339` | `5ec2470d0b5c3f76969ff4f0b944f11521ff74d0` | 댓글 변경 경합(trash·delete·unresolve) | 4개 성공, Web 대기 |
| [#159](https://github.com/AISFlow/fvoci/pull/159) | `00e7dadb17a41cb5ca919ac686237cf26204a6c0` | `2f4483b06411aa85939a18eda8e428684801d2c9` | 문서 메뉴의 instance `features.ai` 설정 소비 | 1개 성공, 나머지 대기·진행 |
| [#161](https://github.com/AISFlow/fvoci/pull/161) | `d76908912c31d1888c117a11d6fb06a75a94dd9a` | `1e7c1a807bca35e50b0cf0fd2e0bf8fc157b5a94` | 프로젝트 문서 리비전 이력 API·UI·forward 복원 | 대기·진행 |
| [#151](https://github.com/AISFlow/fvoci/pull/151) | `27365b68ea02cd0ad75a726e9af1553ae71fe9e6` | `bfc0c5b8f945f1129c0687015ae2707e18ff94ae` | room writer 상실 후 durable 리비전 상태 재적재(stale capture) | 대기·진행 |
| [#163](https://github.com/AISFlow/fvoci/pull/163) | `3870a58e16f5655baf241d0af0f75636249c7b57` | `771d1cf213413258e0637ad42ce0a96799bba768` | 태스크 메타 변경의 같은 프로젝트 내 다른 viewer SSE 전달 | 대기 |
| [#148](https://github.com/AISFlow/fvoci/pull/148) | `62744ab13826ddbb2be9645e84327883a4dddf96` | `8d4b7c3d0fffdb3c295359889b98a30e48f33c45` | 첨부 multipart 완료 재시도·proxy 요청 상한 413/524 | 대기 |

고정 비교 anchor(2026-09-28 KST): main `a01d0829f781b35b9442c2e7f0c844f251d345a6`(#169 merge). 위 기준 main `6ba876503e2a8340c0c21f8f110d2ec14f67f8b3`와
표는 바꾸지 않는다. 아래 PR은 고정 HEAD에서 35개 workflow check(5개 plan·5개 gate 포함, CodeRabbit 별도) 실제 SUCCESS와
별도 Opus 5.5 medium 독립 검토 ACCEPT 후 기대 HEAD로 머지했다. merge 후 main push workflow는 2026-09-27 16:27 UTC에
각 merge SHA에서 일부만 완료됐고 나머지는 대기·진행이다(통과로 표시하지 않는다).

| PR | 고정 HEAD | merge | 범위 |
| --- | --- | --- | --- |
| [#162](https://github.com/AISFlow/fvoci/pull/162) | `548739735922df772b0077128b2a08f972949d7a` | `ed72c41d1e90d212b3c12b05c0f85abd611bd355` | 컬렉션 calendar view drag로 날짜 이동 |
| [#164](https://github.com/AISFlow/fvoci/pull/164) | `a166f9525d13c64919807751614eebfb8ff38edf` | `dec201b347de32db70a00f9272009d65b5bbf240` | 첨부 다운로드의 scope PAT 허용(원본 계약) |
| [#167](https://github.com/AISFlow/fvoci/pull/167) | `79e57029e39bf877762cd6197a5c73de9b97a08a` | `4232daf83b21df62c9523390c6e36b57e44b8c7e` | calendar drag 충돌 검사의 stream 갱신 후 유지 영구 회귀 |
| [#168](https://github.com/AISFlow/fvoci/pull/168) | `02ab157da521105cd6d54da0a01d4945522a1e00` | `d612581dec75a3e3b3386ae14bb50d483cd9e6c9` | 태스크 목록·layout의 `dueBefore`·due 정렬/페이지네이션에 actor 시간대 |
| [#165](https://github.com/AISFlow/fvoci/pull/165) | `c1ee5165d7f460936231e0bf56f6d9592c38738b` | `3046973dbf57bf959cad88e795f24215f2019835` | 서버 메시지 `i18n.overrides` 사용 시점 적용 |
| [#169](https://github.com/AISFlow/fvoci/pull/169) | `cbf0134e8ec282e037e8512f38d2c6da9104e134` | `a01d0829f781b35b9442c2e7f0c844f251d345a6` | 운영 TLS/secure cookie·Rust 이미지 간 업그레이드 절차 문서 |

anchor 이후 추가 수락(anchor는 그대로 둔다): [#170](https://github.com/AISFlow/fvoci/pull/170) 고정 HEAD
`8d4c1dc2d12109184f6d83ce7caa7a915dde88c5`, merge `44e58fa57e6305d31bef914d2f0b4e708b233f37`(2026-09-27 16:35 UTC) — AI 결과의
live 문서 적용·프로젝트 태스크 생성. 35개 workflow check 실제 SUCCESS와 별도 Opus 코드·증거 ACCEPT 후 머지했고,
merge tree는 premerge 통합 tree와 같다. main push workflow는 아직 통과로 표시하지 않는다.

2026-09-28 KST 추가 대조 지점: main `1e0acbe15f903244ff7479c36dce2a90fa6e783d`.
기존 anchor와 당시 관측은 유지한다. 아래 세 PR은 고정 HEAD의 35개 workflow check 실제 SUCCESS와
별도 Opus 5.5 medium ACCEPT 후 머지했고 통합 tree를 대조했다. 이 대조 지점의 main push 전체 검사는
2026-09-27 17:43 UTC에 대기 중이므로 통과로 표시하지 않는다.

| PR | 고정 HEAD | merge | 수락 범위와 실제 검사 |
| --- | --- | --- | --- |
| [#171](https://github.com/AISFlow/fvoci/pull/171) | `ecbcad0782554be6620fd6d23f3b7c8986139462` | `a0c913b7733b23da4f4b47139d978d19cb409622` | 미추출 첨부의 격리 child 즉석 preview, 전달 직전 현재 인가·저장 상태 재검사. native x64/ARM64 인증 경로 3개·test-hang 10개씩, ARM64 preview 11·첨부 25·S3 17개 |
| [#174](https://github.com/AISFlow/fvoci/pull/174) | `9033b93d563ffccece3704b8f0e2f0808afca679` | `92c19c07cb37993e3d27a27f407016198b7aed7f` | 실제 다운로드 bytes의 PDF layout·페이지·확대/축소, same-origin 글꼴/CMap 자산. Chromium 제품 흐름 1개와 한글 렌더링 시각 확인. Office/HWP 수락과 구분 |
| [#172](https://github.com/AISFlow/fvoci/pull/172) | `7d703c2f4693da42551cdcf1d0c59232bb05a4df` | `1e0acbe15f903244ff7479c36dce2a90fa6e783d` | 컬렉션 저장 시간대가 유효하지 않으면 기존 actor 시간대 검증의 UTC fallback 사용. 실제 PostgreSQL collections 8개, 수정 전 500 재현 포함 |

2026-09-28 KST 추가 대조 지점: main `73cc54bac5c978d48018a4d9847fd5415d860b20`(#182 merge). 기존 anchor와 관측은 유지한다.
아래 PR은 고정 HEAD의 실제 check 성공과 별도 Opus 5.5 medium 독립 검토 ACCEPT 후 기대 HEAD로 머지했고, merge tree는
premerge 통합 tree와 같다. 각 merge의 main push workflow는 대기로 관측됐으며 통과로 표시하지 않는다.

| PR | 고정 HEAD | merge | 수락 범위와 실제 검사 |
| --- | --- | --- | --- |
| [#176](https://github.com/AISFlow/fvoci/pull/176) | `64842352c933f33dffc358a2f2e9d3a9bea45588` | `2b828eb2b54fc287ac66738f98fc4cc0e977e55b` | 공개 공유의 명시적 익명 첨부 deep-link 보기 UI(원본 본문 링크·새 탐색 없음). 35개 check SUCCESS, 공유 브라우저 1개. PDF/Office/HWP 수락과 구분 |
| [#177](https://github.com/AISFlow/fvoci/pull/177) | `9d97277e4511f2edf7b99b7200845735ab5a03e3` | `0bd6f7dcb9eb547371c0635a219ac37c379c2a46` | PostgreSQL 16/17의 `uuidv7()` 제품 호환(3파일, migration 불변). 35개 check SUCCESS, 전체+delta 검토. 16/17 전체 CI matrix(#175)와 운영 백업·복구 수락이 아니다 |
| [#180](https://github.com/AISFlow/fvoci/pull/180) | `6243aeedfea6a3dd3b096674f344117a280388e3` | `89d974e2d1696f492cd3e8183ab3401b44e6e9e2` | DOCX 첨부 layout viewer·세션 검색 chunk 보조 표시·ZIP 예산과 숨은 entry 격리. 35개 check SUCCESS, DOCX/PDF/검색 브라우저 각 1개, 전체 검토 BLOCK→delta ACCEPT. Word desktop reflow 동등성은 주장하지 않는다 |
| [#182](https://github.com/AISFlow/fvoci/pull/182) | `b1e198b67732e4d6ff49bf1d6df50ddc755ff1d6` | `73cc54bac5c978d48018a4d9847fd5415d860b20` | 태스크 PATCH의 committed 응답을 상세 cache에 먼저 반영해 다음 편집 유실 방지. NARROW_FRONTEND_WEB_INSTALL 계획의 24개 SUCCESS·5개 SKIPPED(선택 gate 모두 통과), task-edit 브라우저 1개 |

2026-09-28 KST 추가 대조 지점(수락 제품 main): `2caf90d9c076b201e64144a2f1940993f396e470`(#186 merge). 기존 anchor와 관측은 유지한다.
아래 PR은 고정 HEAD의 실제 CheckRun 전부 SUCCESS와 별도 Opus 5.5 medium 독립 검토 ACCEPT 후 기대 HEAD로 머지했고, merge tree는
premerge 통합 tree와 같다(merge SHA는 원격 PR 대조). `2caf90d9`의 main push workflow는 2026-09-27 21:27 UTC에 대기·진행이며 통과로 표시하지 않는다.

| PR | 고정 HEAD | merge | 수락 범위와 한계 |
| --- | --- | --- | --- |
| [#181](https://github.com/AISFlow/fvoci/pull/181) | `ddd8a98532b41137191400054c1566e16bc7eb0c` | `ce2b62daebbad4f200853e125e5779f02536b8b4` | 서로 다른 이미지 간 업그레이드·init 실패 재시도·이전 이미지 복원 smoke. 35개 check SUCCESS, 전체+delta 검토. 실제 실행은 로컬 x64 고정 이미지 쌍뿐이며 ARM64·S3 복원·프로세스 kill은 수락 범위가 아니다 |
| [#183](https://github.com/AISFlow/fvoci/pull/183) | `563f652fa5206ce249218cbab8dd0eec96820b9f` | `ce1ac3a4b854d52615cb991117f03c20391e51f9` | 태스크 이벤트 poll·lease-drop lifecycle 테스트 전제 수정(테스트 전용, 제품 코드·timeout 불변, 기존 단언 유지). 35개 check SUCCESS, event·lease delta 검토 |
| [#175](https://github.com/AISFlow/fvoci/pull/175) | `f38523d92f8d2d7e287e13269b8cfe00feda49d8` | `041d26a8656db129036b6f0127134ed9f0f41bab` | 기존 PG18 x64/ARM64 유지 + PostgreSQL 16.15·17.11 x64 DB suite matrix(digest·`server_version_num` 확인). 39개 check SUCCESS, 16/17 각 a/b shard 303+352개 통과·ignored 0, 구조·통합 검토. PG16/17의 ARM64·운영 백업/복구는 아니며 compose는 18.3 고정 |
| [#178](https://github.com/AISFlow/fvoci/pull/178) | `77d5c7673643787e9ea79ec3daaf5331ef697aa9` | `635065d12ad5e53cdd98c36ed2a2ae7d08644fb1` | HTTP 요청 trace span에 원시 URI 대신 method·경로 템플릿·version만 기록(공유·ICS 토큰, OIDC code/state 제외). 35개 check SUCCESS, 원본·통합 검토. 임의 애플리케이션 로그 전체 감사는 아니다 |
| [#186](https://github.com/AISFlow/fvoci/pull/186) | `82f4fd5970fdcc7c95fc68415d2389f943f58ac2` | `2caf90d9c076b201e64144a2f1940993f396e470` | XLSX 첨부의 실제 셀·시트·병합 셀·페이지·확대/축소 layout viewer(추출 text 아님), 크기·ZIP 한도·Worker 시간 제한/취소, 공유 경로. 35개 check SUCCESS, 원본+delta 검토, 최종 HEAD XLSX 브라우저 1개. Excel desktop 전체 기능·차트·편집은 주장하지 않는다 |

2026-09-28 KST 추가 대조 지점(수락 제품 main): `3919a326f384b2f3168eb495f539a9556b536f73`(#192 merge). 기존 anchor와 관측은 유지한다.
아래 PR은 고정 HEAD의 실제 CheckRun 전부 SUCCESS(#192는 opt-in ARM job의 일반 PR 실행 skip 제외)와 별도 Opus 5.5 medium
독립 검토 ACCEPT 후 기대 HEAD로 머지했다(merge SHA는 원격 PR 대조). `3919a326`의 main push workflow 5개는
2026-09-27 23:54 UTC에 queued이며 통과로 표시하지 않는다. PR 수락 근거와 merge 후 main push CI는 구분하며, 이 표와 이전 anchor의
main push 대기 관측은 이미 수락된 PR의 취소가 아니다.

| PR | 고정 HEAD | merge | 수락 범위와 한계 |
| --- | --- | --- | --- |
| [#187](https://github.com/AISFlow/fvoci/pull/187) | `0e3e95ddeaaf24582f1c83b29dccd2e793a2dd4c` | `8f4c40ef27e82cd8619a68eff77a9485144530f6` | HWP/HWPX 첨부의 취소 가능한 문서 Worker layout 보기(열기 취소·확장 크기 한도), XLSX·고지 변경과의 통합 tree 검토. 39개 check SUCCESS. HWP 편집·사본 저장(#189)은 포함하지 않는다 |
| [#190](https://github.com/AISFlow/fvoci/pull/190) | `fdab3903beb4b32a980d52c153d62e0d0610ec7c` | `217cc2a594b1ff2fb1278524bd59ef899489398a` | S3 저장소 이미지 업그레이드·versioned rollback smoke 스크립트와 운영 문서. 로컬 S3 호환 silo·x64 실제 실행, 전체+delta 검토(문서 BLOCK→delta ACCEPT), 39개 check SUCCESS. 실제 클라우드 제공자·ARM64 S3는 아니다 |
| [#194](https://github.com/AISFlow/fvoci/pull/194) | `e8e1909d9eb923baeceb6646192b3d0aeee4b7d2` | `f1934ad0406356394076ad357ca992ad7bed243a` | lease 축출 후 늦은 Leave 무시와 backpressure 축출 뒤 session 리비전 trigger 보존(검토 CHANGES REQUESTED→delta ACCEPT). 39개 check SUCCESS, hosted ARM64 [collaboration-arm64](https://github.com/AISFlow/fvoci/actions/runs/36354409414/job/108721208498)의 `revision_integration` 31/31 |
| [#195](https://github.com/AISFlow/fvoci/pull/195) | `2883f80313dcfce1bde78c2d2e9867d45b6a2891` | `e581d08be2c5723d6eb0d9c1f828b528f7b92214` | 격리 Playwright 실행의 실패 산출물(error-context·trace) 보존과 종료 코드 전파. 실제 Playwright 실패/성공 fixture가 비0 종료·보존을 확인하며 기존 테스트 단언·timeout은 약화하지 않았다. 39개 check SUCCESS, 검토 ACCEPT |
| [#188](https://github.com/AISFlow/fvoci/pull/188) | `0612a64981db70310ab33f28bc334df3ec91f0fd` | `6fe9dbe7a48a386282cf30a0873115c2f10cbfc8` | PPTX 첨부의 bounded slide Worker layout 보기, placeholder 경계 안 fallback 표시. 최종 후보 `0612a649` Opus placeholder 경계 검토 ACCEPT, 원래 fixture 위치의 원격 [web-browser-shard-6](https://github.com/AISFlow/fvoci/actions/runs/36356910156/job/108726829404) PASS(1/1), 39개 check SUCCESS. PowerPoint desktop 동등성·애니메이션·편집은 주장하지 않는다 |
| [#192](https://github.com/AISFlow/fvoci/pull/192) | `1f0addd8a76c9439d0d9e4cb7b4d3782c08684f1` | `3919a326f384b2f3168eb495f539a9556b536f73` | 수동 opt-in native ARM64 이미지 업그레이드 CI job. 수동 실행 [36351751670](https://github.com/AISFlow/fvoci/actions/runs/36351751670)의 `upgrade-smoke-arm64`가 고정 쌍 `d0942f10`→`1e0acbe1`(local storage)을 실제 ARM64에서 통과하고 `install-ci-gate` SUCCESS. 다시 연 PR의 선택 check 모두 SUCCESS, 일반 PR 실행의 ARM job은 opt-in이라 not applicable(skip). S3·다른 이미지 쌍·최신 제품 이미지는 아니다 |

실제 OS IME witness(F, 협업 행): 별도 Opus 5.5 medium 검토가 ACCEPT했다. 비공개 WSL2 X11 silo(IBus hangul 2벌식,
headed Chromium 153)에서 실제 XTEST 키 입력으로 preedit 중 다른 문단의 원격 갱신(조합 유지), preedit Backspace와 조합 후
Backspace/undo(원격 편집 보존), 제품 저장 버튼의 persist ACK, graceful 서버 재시작 후 새 컨텍스트 재열기를 2/2 통과했다.
provenance: 실행 source `0e3e95dd`의 frontend(`apps/web`·`packages`)는 수락 main `e581d08b`와 차이가 없고, backend는
#194로 수락된 `e8e1909d`의 정확한 rebuild다. main에서 직접 빌드한 것이 아니다. 이 witness의 한계로 Windows·macOS·모바일 IME,
같은 node 삽입 경합, crash 중 입력은 포함하지 않는다(새 필수 조건이 아니다). helper SIGKILL·actor panic 복구는 #131·#114로 별도 수락됐다.

[#149](https://github.com/AISFlow/fvoci/issues/149)의 `fvoci` 계정 편집 이력에는 A(API 프록시 유지) 선택
(2026-09-27 15:59:53 UTC)과 승인 체크(16:00:03 UTC)가 있다. 자동화도 같은 계정을 사용하므로
사용자의 직접 선택인지는 2026-09-28 KST에도 확인되지 않았고 체크 표시로 승인을 추정하지 않는다. 확인 전 현재 수락된 API 프록시를 유지한다.
이 이슈를 정책 확정으로 닫았던 기록은 정정하고 다시 열었다. 제품 변경은 없으며,
#148의 프록시 회귀는 유효하고 실제 Cloudflare 경유는 미실행이다.

검증 기준: 각 PR의 필요한 실제 검사·원격 CI와 별도 세션의 독립 검토를 고정 SHA에서 확인한다.
과거 Opus/Fable 검토는 당시 범위의 근거로 보존하며, 현재 역할은 AGENTS.md를 따른다.
최신 실행·소유권·검증 SHA·인계 포인터는 `/home/kinesis/orca/fvoci-evidence/coordinator-handoff-2026-09-26.md`에 둔다.

현재 작업·잔여 범위:
- 기준 main 시점에 수락 대기였던 #142·#146·#148은 이후 위 표의 고정 HEAD로 수락·머지됐다. 당시 관측은 git 이력에 보존한다.
- 2026-09-27 14:55 UTC 관측의 수락 대기 #162·#164·#165는 이후 위 anchor 표의 고정 HEAD로 수락·머지됐다(#165는 당시
  `7f3e24779f967092cfdb1b901edffa10dd48b618`에 서식 전용 delta를 더한 `c1ee5165`). 당시 관측은 git 이력에 보존한다.
- anchor 시점 C였던 AI 결과 적용 PR #170은 위와 같이 수락·머지됐다.
- 이전 C/B였던 서버 즉석 preview(#171), PDF viewer(#174), 컬렉션 저장 시간대 F1(#172)은 위 고정 SHA로 수락됐다.
- 이전 C/B였던 공유 첨부 deep-link(#176), PG16/17 UUID 호환(#177), DOCX viewer(#180)와 태스크 PATCH cache(#182)는
  위 `73cc54ba` 표의 고정 SHA로 수락됐다.
- 이전 C였던 #175·#178·#181·#183과 XLSX viewer(#186)는 위 `2caf90d9` 표의 고정 SHA로 수락됐다. 당시 관측은 git 이력에 보존한다.
- 2026-09-27 21:27 UTC 관측의 C였던 HWP viewer(#187), PPTX viewer(#188, 당시 `d1c7cbf1` → 최종 `0612a649`),
  S3 업그레이드 witness(#190), ARM64 업그레이드 job(#192)과 이후의 #194·#195는 위 `3919a326` 표의 고정 SHA로 수락됐다.
  당시 관측(#192 run queued 등)은 git 이력에 보존한다.
- 수락 대기(C, 2026-09-28 KST 관측; 로컬 검사·독립 검토 통과와 PR 수락은 구분): HWP 편집·인가된 사본 저장
  [#189](https://github.com/AISFlow/fvoci/pull/189)의 수락 main 통합 후보 `cd70a22d`는 별도 Opus composition 검토 ACCEPT와 로컬 좁은 검사가
  있으나 현재 원격 CI(관측 시 queued)와 코디네이터 수락 전이다.
  수락으로 표시하지 않으며 기존 PDF·DOCX·XLSX·PPTX·HWP 보기·변환·백업 수락 범위를 취소하지 않는다.
- Web Push: 실제 제공자 실행 9회차에서 `AbortError: Subscription failed - no active Service Worker`가 구체 제품 결함으로
  확인됐다. 수정은 C [#196](https://github.com/AISFlow/fvoci/pull/196)(`ec3da98d`; 독립 검토·원격 CI·실제 headed witness 대기, 미수락)이고,
  실제 외부 전달·브라우저 수신·로그아웃 해제는 별도 F로 witness가 아직 없다.
- 실제 외부 IdP·인증 앱 스캔 F는 사용자 환경(계정·기기)이 필요한 실행 증거이며 코드 정책 선택 문제가 아니다.
- 분류 근거·행별 종료 조건: `/home/kinesis/orca/fvoci-evidence/opus-decision-status-cleanup.md`.
- 문서 변환의 정상 Rust 경로는 수락됐으며 Node 구현 복원은 필요 없다. 개발·CI의 Node/Python과 독립 reader는
  제품 런타임과 별개다. 필수 백업·복원 자체 검증은 Rust이며 pg_dump/pg_restore·얇은 실행 스크립트는 유지한다.
- migration 037·038·039·040은 main에 수락됐다. 번호를 다시 배정하지 않는다.
- 잔여 기능·검증: 실제 브라우저 PushManager 및 외부 푸시 서비스(FCM 등) 발송(F),
  #77 외부 제공자 검증의 미실행 범위(F), wiki 컬렉션 권한 N+1(#76 S4 비차단 후속, 기능 차단 아님: `3919a326`의 `src/db/collection_query.rs`에서 비관리자 문서별 `document_permission` 반복 확인; S3 `dueBefore` 시간대는 #168 수락),
  HWP 편집·사본 저장(C #189; HWP 보기 #187·PPTX #188·DOCX·세션 chunk 보조 표시 #180·XLSX #186은 수락),
  추가 DB(G 추적). 수락(`3919a326`): S3 이미지 업그레이드·versioned rollback 로컬 S3 호환 silo x64 실행(#190; 실제 클라우드
  제공자 미검증), ARM64 이미지 간 업그레이드 고정 쌍 `d0942f10`→`1e0acbe1` local storage 실제 실행(#192; S3·다른 쌍 아님),
  실제 OS IME witness(위 범위 한정). 이미지 간 업그레이드·init 실패 복구 로컬 x64(#181)와 PostgreSQL 16/17 x64 matrix(#175; UUID 호환 #177)는 수락.
  수락(anchor): `i18n.overrides`(#165), 태스크 목록·layout `dueBefore` actor 시간대(#168), calendar drag(#162·#167), 첨부 PAT 다운로드(#164),
  운영 TLS·업그레이드 안내 문서(#169); anchor 이후 AI 결과 적용 UI(#170).
  후속(비차단): #83 `DeriveFailed` dead arm·patch-block 409 테스트·duplicate node cap·backlinks references table;
  `BRANDING_ASSET_MAX_BYTES`의 settings 모듈 이동(현재 512 KiB 한도는 원본과 동일); #82 hybrid lexical leg 필터 차이; #85 S1/S2·
  EPUB/HTML 추출; #88 S3 time_entries 명시 grant 줄; #91 admin
  erase ConfirmActionButton key/portal; #92 process-wide engine cap last-write-wins·spawn_room CapacityRetry dead path;
  #97 64 KiB 토큰 응답 통합 케이스·RFC 3339 updated_at 벡터·ambiguous-kid/typ/cty 벡터·RUNNING.md 문구; #99
  RUNNING.md FVOCI_COLLAB_MAX_CHILDREN 문구. 보상 실패 참조 소실은 #112에서 수정·수락했다.
  의도적 차이: 가져오기 비동기 실행은 가져온 사용자의 세션 필요, 본문 한도 JSON ≈85 MiB(디코드 64 MiB); #78 purge
  저장소 먼저; #110 유효 사용권이 없을 때 저장 quota 기본 무제한; #83 backlinks 파생·쓰기 거부 404; #85 HWP는 helper 없으면 skipped; #96 Tiptap
  126단계 초과는 400(이전 500); #97 추가 audience·kid 없는 다중 키 토큰 거부, RSA>4096 미지원, 비표준 claim 타입 거부;
  #99 collab engine 없으면 duplicate 503.

범위 결정(사용자 확인 2026-09-25): 기존 TypeScript FVOCI 배포가 없으므로 TS 데이터 이전(스키마 변환·사용자/세션/토큰
이전·문서 corpus 일괄 이전·dual-write·TS 복귀)은 범위 밖이다. 신규 설치·Rust 스키마 migration·Rust 저장 데이터의
재시작/crash 복원·백업/복구·업그레이드와 제품의 문서 가져오기/내보내기는 범위다.

협업 room 수 구분: 제품 기본값 30, compose 설치 64(PostgreSQL max_connections 150), 검증 64 room×2 사용자 180 s
(단일 WSL2 호스트), 설정 상한 512는 미검증.

## 3. 기능 대응표

상태: 수락(main 반영·검증) / 부분(일부 경로 수락) / 진행 / 미착수. "남은 차이"는 항목별로 진행·미착수·미검증·
후속(수락 범위의 비차단 개선)으로 구분하며, 전체 포팅 완료 전
해결하거나 사용자가 승인한 차이로 기록해야 하는 항목이다.
부분 행의 잔여 분류(기준 main `6ba876503e2a8340c0c21f8f110d2ec14f67f8b3`, 2026-09-27): A 오래된 표기(이미 수락) ·
B 명확한 구현·문서 작업 · C 수락 대기(열린 PR·진행 워커) · D 결함 · E 사용자 정책 결정 필요 · F 외부 환경 검증 필요(생략 불가) ·
G 선택·미승인·원본 부재(새 요구 없이 만들지 않음). 각 행의 "종료:"가 그 행을 닫는 조건이다.

| 기능 | 원본 근거 | 보존할 불변식 | 상태 | 증거 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·로그인·세션·프로필 | identity/routes.ts, core/auth.ts | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | 부분 | #1, #34, #53, #69, #72 | 수락: 비밀번호 재설정(#53), 탈퇴·익명화·비밀번호/이메일 변경·magic link·export(#69), 설정 기반 비밀번호 최소 길이(#72), TOTP MFA·OIDC·workspace SSO(#77), MFA 등록 QR(SVG)·수동 키 UI(#84). F: 실제 인증 앱 스캔, #77 실제 외부 IdP 미실행 범위(브라우저 구조 검사와 구분; 사용자 환경 필요, 코드 정책 선택 아님). 종료: F 실행 증거 |
| 워크스페이스 | domains/workspaces | 현재 역할·철회 경합·RLS·풀 컨텍스트 | 수락 | #4, #39, #56, #61, #63, #110, #134 | 수락: counts·owner 전용 삭제(#56), 30일 purge 실행기(#61), S3 저장소 purge(#63), 원본 사용권별 workspace/guest/storage quota(#110), workspace ZIP 내보내기(#134). A: 남은 차이 없음 |
| 멤버·초대 | invitation.ts, quota.ts, consent.ts | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | 수락 | #21, #53, #69, #72, #77, #84 | 수락: 초대 메일(#53), 초대 수락의 legal consent 428·`defaults.user` 적용(#72), 탈퇴·관리자 삭제 시 보낸 pending 초대 정리(#69·#84), 초대 수락 시 MFA challenge·OIDC 초대 수락(#77). A: 알림/사용자 기본값·계정 삭제 정리·E2E SQL fixture(없음). G: pending 목록/철회 API(원본 contracts·UI에 없음) |
| 그룹·권한 통합 | policies.ts effectivePermission, project/document_members(user XOR group) | 리소스별 단일 권한 함수 | 수락 | #23, #39, #50 | 후속: collab 프레임당 권한 재조회 축소·collab_delivery의 그룹 join 사본·설정 UI `canManage` DTO |
| 프로젝트 | domains/projects | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | 수락 | #13, #39, #52, #76, #78, #80, #83, #161 | 수락: 프로젝트 문서 협업·trash/restore/sort·archive·30일 purge(#78), 프로젝트 문서 body/children/ancestors/backlinks/duplicate(#83), collection/view 복제(#52·#76), 프로젝트 문서 첨부(#80)·태그 route. 기존 B 표기 정정(2026-09-27): 프로젝트 문서 리비전 API·UI·forward 복원은 #161 수락, 프로젝트 그룹 route(`src/http/routes/groups.rs`)·UI(`project-groups.tsx`, `workspace-groups-flow` E2E)는 #39에서 이미 수락(A). A: 남은 차이 없음 |
| 태스크 | domains/tasks, core/task.ts, workflow.ts | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | 수락 | #13, #19, #26, #38, #40, #47, #60, #142, #163, #168, #182 | 수락: activity feed(#60), 본문 협업·block patch·수동 리비전(#104), origin UI/API(#117), archive 전 본문 영속화(#121). Gantt 조회·편집 UI(#130)는 수락. 수락(기준 이후): board 그룹 paging·drag-and-drop(#142), 태스크 메타 변경 SSE 전달(#163), 태스크 목록·layout의 `dueBefore`·due 정렬/cursor actor 시간대와 잘못된 저장 시간대의 UTC fallback(#168), PATCH committed 응답의 상세 cache 반영으로 다음 편집 유실 방지(#182). A: 남은 차이 없음(컬렉션 query의 저장 시간대 검증 D는 공유·컬렉션 행) |
| 일정·ICS·휴일 | routes.ts ics/holidays | 일정 의미 | 수락 | #49, #76 | 수락: 휴일·ICS 피드(담당 태스크, #49), 저장 calendar view 기반 ICS 분기(#76). A: 남은 차이 없음 |
| 위키 문서 | domains/documents, core/document.ts | 현재 문서 권한·트리 잠금 순서 | 수락 | #5, #23, #39, #66, #70, #78, #83 | 수락: 가져오기·내보내기(#66), 공유 링크(#70), trash 30일 purge(#78), body/block patch/children/backlinks/duplicate/flat(#83). 수락: office·Notion 가져오기(#85), Rust 변환·내보내기/doctor·Node 없는 제품 이미지(#101·#103·#106·#107). 수락: 가져오기 복구 보상(#112), 문서·태스크 템플릿(#118). A: 남은 차이 없음 |
| 리비전 | documents/revisions.ts, core/revision.ts, collab applyRestore | 복원은 room actor의 forward system update, durable 후 broadcast | 수락 | #25, #104, #123, #128, #151, #161, #194 | 수동·session·scheduled 리비전과 자동 보존 개수 제한은 수락. A: 리비전 route의 `document_permission` 인가. 기존 D 의심(writer-stale 후 연결 없는 room의 수동 캡처가 오래된 본문 저장): #151에서 실제 앱 역할 PG·WebSocket 재현 후 수정·수락. 프로젝트 문서 리비전(#161) 수락. A: 남은 차이 없음 |
| 댓글 | comments/routes.ts, core/comment.ts | 문서 XOR 태스크·부모 활성·권한 | 수락 | #28, #47, #58, #146 | 수락: 그룹 멘션·프로젝트 문서 댓글(#58; 그룹 멘션은 원본 snapshot과 달리 전달 시점 재전개). 수락(기준 이후): 이중 DELETE 중복 이벤트·resolve 경합·동시 trash된 태스크 댓글(#146). A: 남은 차이 없음 |
| 협업 | domains/collab, React/Tiptap | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | 수락 | #6, #7, #18, #24, #27, #39, #46, #114, #131, #194 | 수락: room 용량(기본 30, 64 검증; §2), helper SIGKILL 후 복구(#131). A(오래된 표기 정정): actor panic/rejoin(1011 종료·자원 해제·durable 상태 재적재 후 successor 1개)은 `collab_lifecycle` 20개의 panic/rejoin 명명 테스트가 #114 x64/ARM64 협업 실행에서 통과해 수락됐다. 제품 이미지·compose는 `FVOCI_COLLAB_ENGINE`을 기본 설정하고 단독 서버는 그 env가 필요하다(정책·코드 변경 없음). 수락: lease 축출 후 늦은 Leave 무시·backpressure 뒤 session 리비전 보존(#194). 수락(기존 F 종료, 별도 검토 ACCEPT): 실제 OS IME witness(Linux X11 IBus hangul 2벌식·Chromium 153 실제 XTEST 입력, preedit 중 원격 갱신·Backspace/undo·persist ACK·graceful 재시작 후 재열기; §2). 한계(새 필수 조건 아님): Windows·macOS·모바일 IME, 같은 node 삽입 경합은 이 witness 범위 밖이다. A: 남은 차이 없음 |
| 첨부 | domains/attachments, packages/storage | 부모 권한·원본 bytes·원자 완료·취소 | 부분 | #10, #39, #58, #63, #65, #80, #148, #176, #180, #186, #187, #188 | 수락: viewer route(#58), S3 프록시·중단 업로드 GC(#63, #65), 태스크·프로젝트 문서 부모·DELETE·quota·이미지 preview·저장 추출문 preview-html(#80). 수락: `--verify-storage` preview 객체·크기 확인(#90). E: S3 전송 정책의 사용자 직접 선택 확인([#149](https://github.com/AISFlow/fvoci/issues/149)); 확인 전 API 프록시 유지. 수락: 미추출 파일의 preview-html 즉석 격리 parse(#171), PDF layout viewer(#174), 공개 공유 첨부 deep-link UI(#176), DOCX layout viewer·세션 `?chunk` 보조 표시·ZIP 예산/숨은 entry 격리(#180; Word desktop reflow 동등성 아님). 수락: XLSX layout viewer(#186; Excel desktop 동등성·차트·편집 아님). 수락: HWP/HWPX viewer(#187), PPTX viewer(#188; PowerPoint desktop 동등성 아님). C: HWP 편집 사본 저장(#189, 통합 후보 `cd70a22d`); 추출 plain text는 layout viewer와 동등하지 않다. 수락(기준 이후): 업로드 완료 재시도·요청 상한 413/524(#148), 다운로드의 scope PAT 허용(#164, 기존 D 해소). 종료: E 확인·결정 반영, C 수락 |
| HWP/HWPX 추출 | 원본 추출 경로, rhwp e8800c8 | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | 수락 | #2, #8, #9, #11, #35 | A(오래된 표기 정정): lease 만료·재시도 결과 게시 경계는 #11의 lease token 게시 fence로 수락됐고, 탈취된 lease의 finish 0행·재시도 소진 `worker_failure`를 실제 PG 회귀가 확인한다(아래 A 근거). G: HWP 썸네일(원본 thumbnail은 이미지 MIME만, 새 요구로 만들지 않음) |
| 검색·색인·AI | domains/search, packages/search | 검색에서도 인가·철회·색인 복구 | 수락 | #29, #30, #35, #48, #57, #58, #82, #83, #159, #170 | 수락: 워크스페이스·전역 검색, 댓글 hit, outbox 색인 배치(#57), 복구 후 rebuild, PAT scope 검색(#83), 의미(벡터) 검색(#82), 첨부 hit의 viewer 이동(#58, search E2E). A: 검색·색인·의미 검색은 수락. 수락(기준 이후): 문서 메뉴의 `features.ai` 소비(#159). 수락(anchor 이후): AI 결과의 live 문서 적용·프로젝트 태스크 생성 UI(#170, 기존 B 해소). A: 남은 차이 없음 |
| 알림·outbox·메일·webhook·연동 | domains/notifications, packages/jobs | 커밋 후 전달·중복/재시도 | 부분 | #31, #35, #45 | 수락: 앱 내 알림, 메일·digest(#53, #61), webhook·GitHub App·AI 동작(#74). 수락: Web Push와 로그아웃 시 브라우저 연결 해제(#139). C: 9회차 no active Service Worker 결함 수정(#196). F: 실제 외부 푸시 서비스 전달·브라우저 수신·로그아웃 해제. G: requeue 운영 API/UI(원본도 없음). 종료: C 수락, F 증거 |
| 공유·즐겨찾기·최근·태그·컬렉션 | 해당 routes | 공유 링크 권한 | 수락 | #70, #72, #76, #84, #138, #142, #162, #167, #168 | 수락: 즐겨찾기·최근·공유 링크·공개 공유 페이지·PDF, 공유 정책(#72), 태그·컬렉션·저장 view(#76), 대화상자 정책·`/s/:token` head meta(#84), 공유 첨부 preview(#80). 수락: `HEAD /s/:token` 보안 헤더·빈 본문(#138). 수락(기준 이후): board 그룹 paging·drag-and-drop(#142), calendar view drag(#162)와 stream 갱신 후 충돌 검사 영구 회귀(#167), 태스크 목록·layout의 `dueBefore` actor 시간대(#168; 컬렉션 query는 이미 actor 시간대). 수락: 컬렉션 저장 시간대 UTC fallback(#172, #168 검토 F1 해소). 수락: 공개 공유 첨부 deep-link UI(#176) |
| 동의·감사·사용권·관리 | legal, auth.consents, admin.audit, packages/ee | 동의 gate·증거·권한 | 수락 | #72, #84, #159, #165 | 수락: 관리 API·instance settings·법률 문서·동의·428 gate·branding(#72), 관리자 사용자 삭제 예약/취소(#84). 수락: 사용자가 원본 정책 보존을 확정한 사용권·quota(#110), 운영자 정보(#119), security.txt(#120), 브라우저 오픈소스 고지(#122). 수락: settings `embed`(#124)·`attachmentPreview`(#80) 소비, 문서 메뉴 `features.ai` 소비(#159), 서버 메시지 `i18n.overrides` 사용 시점 적용(#165). G: 사용권 issuer trust(원본도 비어 있음) |
| 제품 MCP·CLI·백업·복구 | init.ts, backup.ts, doctor.ts, MCP | 프로토콜·복원 | 수락 | #32, #31, #35, #63, #81, #84, #90, #109 | 수락: 컨테이너 설치 백업·복구(outbox cursor 재기준·검색 rebuild 포함), S3는 스크립트 백업 거부·복구 후 `--verify-storage`(#63, branding #84). 수락: 제품 MCP·CLI·doctor(#81), 키 fingerprint·`--verify-secrets`(#90), Rust 백업 manifest/preflight·키 검증 공유(#109). 개발 Python oracle는 유지. A: 남은 차이 없음 |
| 설치·배포 산출물 | infra/app, compose | 비특권 실행·migrate/grant 분리·helper 포함 | 수락 | #17, #20, #29, #32, #35, #169, #175, #177, #181, #190, #192 | 수락: 운영 TLS/secure cookie 안내와 같은 volume의 이미지 간 업그레이드·init 실패·이전 이미지 복귀 절차 문서(#169), PostgreSQL 16/17 `uuidv7()` 제품 호환(#177), 이미지 간 업그레이드·init 실패 재시도·이전 이미지 복원 smoke(#181, 로컬 x64 고정 이미지 쌍), PostgreSQL 16/17 x64 DB suite matrix와 기존 PG18 ARM64 유지(#175; PG16/17 ARM64·운영 백업/복구 아님). 수락: S3 이미지 업그레이드·versioned rollback 실제 실행 witness(#190, 로컬 S3 호환 silo·x64), 수동 opt-in ARM64 이미지 업그레이드 job과 실제 ARM64 실행(#192, 고정 쌍 `d0942f10`→`1e0acbe1` local storage). 기존 C·F 종료. 수락 범위의 한계(새 F 아님): 실제 클라우드 S3 제공자, ARM64 S3·다른 이미지 쌍·최신 제품 이미지. A: 남은 차이 없음 |
| 추가 DB·플랫폼 | PR999 packages/db (SQLite/libSQL/Turso) | 원본 제공 범위와 목표 구분 | 미착수 | — | PG 우선; 원본 다중 DB를 완료로 간주하지 않음. G(추적 유지): 원본도 SQLite·libSQL·Turso를 목표로만 두고 전체 앱 백엔드로 선택할 수 없음. 제외하지 않으며 원본 계약 범위가 정해지면 B |
| 프론트엔드 | apps/web, packages/editor | 한국어·접근성·기존 흐름 | 부분 | 각 PR E2E | 수락: 첨부 HWP viewer(#187)·PPTX viewer(#188), 격리 Playwright 실패 산출물 보존(#195). C: HWP 편집 사본 저장 UI(#189). 종료: C 수락 |

A 전환 근거(수락 PR merge, 2026-09-27 대조):
워크스페이스 [#56](https://github.com/AISFlow/fvoci/pull/56) `179a96e14ff66a41f5140b3feb0eb4e17af8b43c`,
[#61](https://github.com/AISFlow/fvoci/pull/61) `b0d40ea3c41184065d93703fdd97a3751c07c646`,
[#63](https://github.com/AISFlow/fvoci/pull/63) `7c10edb04336f1c085cfff0d30b8d2ad139fab72`,
[#110](https://github.com/AISFlow/fvoci/pull/110) `30c520022a7a37a8351ffc4f32aad681568c233e`,
[#134](https://github.com/AISFlow/fvoci/pull/134) `bf3ad9c4ab07008e264d5e8664fa675972409da3`;
멤버·초대 [#69](https://github.com/AISFlow/fvoci/pull/69) `4afb18dcce261cab619f4237e57ae7f70bce9423`,
[#72](https://github.com/AISFlow/fvoci/pull/72) `4c46252a9883d5ccfd59ee5e84945c2cfec1f890`,
[#84](https://github.com/AISFlow/fvoci/pull/84) `90a3df02afed4e4531f6fee76dfa724600c2dd9e`;
ICS·프로젝트 view 복제 [#76](https://github.com/AISFlow/fvoci/pull/76) `0cf6851286169bf600fedb1332b07dcda16f3c89`,
[#52](https://github.com/AISFlow/fvoci/pull/52) `3e79f549bd7ab167a01fea9cf7f04697c1e8cf4d`,
프로젝트 문서 첨부 [#80](https://github.com/AISFlow/fvoci/pull/80) `a69e623bc738258db6be2f39bb91855df4ab7f44`;
위키 문서 [#83](https://github.com/AISFlow/fvoci/pull/83) `28a86f13cfc6a1daad5fe6ee04959c645ba42665`,
[#112](https://github.com/AISFlow/fvoci/pull/112) `fee40552abc751bd5ecbf9e8d66f04fc998b9338`,
[#118](https://github.com/AISFlow/fvoci/pull/118) `36ce72f794c7e04baaf6124710b163b206289f35`;
리비전 인가·프로젝트 그룹 route/UI [#39](https://github.com/AISFlow/fvoci/pull/39) `ae8fc1fff9e4a964da412bba05cc9c3111d1e85a`;
검색 [#58](https://github.com/AISFlow/fvoci/pull/58) `cccec451911ce48a3bb1535beb2f76c23155b43f`,
[#82](https://github.com/AISFlow/fvoci/pull/82) `926f01dc01f2fca99bae8df3e02ccdc09819922e`;
MCP·CLI·백업 [#81](https://github.com/AISFlow/fvoci/pull/81) `f89e9eafb0bddb237a2c34f7ea836f331ed64e62`,
[#90](https://github.com/AISFlow/fvoci/pull/90) `d0942f10f89248b385eeb3f83475dfa4cd441388`,
[#109](https://github.com/AISFlow/fvoci/pull/109) `14fb7d3d106baac279c641359d197c7606cbe517`.

A 전환 근거(anchor `a01d0829`, 2026-09-28 KST 대조):
HWP/HWPX 추출 lease 경계 [#11](https://github.com/AISFlow/fvoci/pull/11) `006943bc22328ce05c8516d0192a3741f8a1531e` —
`finish_extract`는 lease token·`pending` 조건이 맞는 행만 한 트랜잭션에서 게시하고, 만료는 재claim만 허용한다.
`tests/attachment_extract_integration.rs`의 `stale_lease_finish_matches_zero_rows`·`retry_exhaustion_marks_worker_failure`가
[#164 postgres-b](https://github.com/AISFlow/fvoci/actions/runs/36323630111/job/108634939247)(HEAD `a166f952`)에서 통과했다.
작업 루프의 중첩 writer·태스크 부모·hard delete 추가 회귀는 선택 사항이며 재현된 결함이 아니다.
협업 actor panic/rejoin [#114](https://github.com/AISFlow/fvoci/pull/114) `0e7a0dc18f11b4030d9975039257cab5690cb423` —
`tests/collab_lifecycle.rs` 20개(panic teardown·guard barrier·durable 상태 보존·rejoin 복원·동시 rejoin 단일 actor 포함)가
x64/ARM64 협업 실행에서 통과; helper process tree SIGKILL 후 새 컨텍스트 복구는
[#131](https://github.com/AISFlow/fvoci/pull/131) `8b75d7f965edc9da6bc448cbca455cc0a3d454c6`(hosted `collaboration-flow` 26개 통과).
실제 OS IME는 이 근거에 포함되지 않으며 §2 witness로 별도 수락됐다.

## 4. 유지하는 결정과 의도적 차이

- 작은 단일 Rust 서버(axum·SQLx) + PostgreSQL, 협업은 Yrs native engine, HWP는 rhwp native helper.
  프레임워크·CRDT·HWP 구현체를 다시 비교하지 않는다.
- DB 역할: 소유자 URL은 `fvoci-migrate`와 `--grant-app-role`만 사용한다. 서버는 `DATABASE_APP_URL`만
  받고 기동 시 스키마 버전(`schema_migrations`, 앱 역할 SELECT 전용)이 컴파일 버전과 같아야 한다(#20).
  권한 적용은 단일 트랜잭션이며 수락된 migration 파일은 SHA-256으로 고정한다(#16).
- 협업은 `FVOCI_COLLAB_ENGINE` helper가 있을 때만 켜진다. 제품 이미지·compose는 이를 기본 설정하고 단독 서버는
  env가 필요하다. 신뢰하지 않는 문서 parser는 요청 처리 프로세스와 분리된 helper다.
- 표준 구현 재사용(2026-09-26 평가, `fvoci-evidence/std-impl-eval-mcp-totp-20260926.md`): 제품 MCP는 손으로 쓴
  stdio JSON-RPC를 유지한다 — rmcp 3.4.1 기본값이 parse 오류 -32700, tool error 형태, schema draft, 프로토콜 버전
  집합에서 원본 TS 서버·현재 클라이언트 계약과 달라 통합 테스트가 깨지며 절약이 작다(고정 버전 adapter로 재검토).
  Fable 검토로 확정(`review-std-impl-eval-mcp.md`): -32700 응답과 16 MiB stdin 캡은 원본(SDK 1.30은 파싱 불가 줄을
  버림)을 넘는 FVOCI 강화이며, 고정 버전 rmcp adapter는 캡 복원 때문에 107줄 루프보다 커진다(130–170줄); Claude Code·
  Cursor는 2025-11-25를 협상하고 Claude Code는 먼저 server/discover를 탐색하므로 향후 rmcp 시도는 그 경로부터 확인한다.
  TOTP는 부분 재사용으로 확정해 #98에서 코드 계산·검증·base32만 totp-rs에 맡기고 otpauth URI·recovery·seal·claim_step
  replay는 FVOCI가 유지한다. OIDC/JWT는 #97에서 `openidconnect`로 대체했다(SSRF 가드 client 유지, 자체 검증기 삭제).
- 의도적 원본 차이: 프로필 변경 감사 기록 추가, 세션 철회 중 쓰기 차단 강화, 429 계약 정합,
  엄격한 ISO 날짜(원본 `z.iso.date()`와 동일 수용 집합).
- 검색은 사용자 결정으로 원본과 같은 Meilisearch를 쓴다. 서버는 index 범위 scoped key만 받고(마스터 키는
  init만), Meili 필터는 recall 최적화이며 보안 경계는 PG hydrate다. Meili가 없으면 서버는 기동하고 검색만
  503을 낸다(키 거부 401/403만 기동 실패).
- 협업 helper는 신뢰할 수 없는 Yrs 디코더 격리 때문에 room당 프로세스를 유지한다(I1–I5). 용량은 개수 상한
  대신 메모리 예산 admission과 oom_score_adj로 늘린다(자문 Q4).
- 동시 쓰기 워커는 코디네이터 직접 구현·하위 agent를 포함해 프로젝트 전체 최대 5(2026-09-27 사용자 승인; 이전 3은 #140 기록 보존), 무거운 로컬 검사는 한 묶음,
  worktree별 target, 실행별 DB/역할/포트를 유지한다. 읽기 전용 검토·조사는 쓰기 슬롯에서 제외한다.
  독립 검토는 별도 세션이며 코디네이터 자기 검토를 독립 검토로 표시하지 않는다.

## 5. 알려진 결함·위험

- 협업 caret [1,1] 간헐 실패는 #24(awareness 갱신이 native caret을 덮는 제품 결함)로 수정했다. ACL poll이
  한 틱씩 건너뛰던 결함은 #27로 수정했다(철회 지연 2배 → 설정값).
- 협업 room마다 소유 fence(advisory lock)용 PG 연결(풀 밖)을 하나씩 점유한다. 필요 연결 = room 수 + 앱 풀 + reserve 10이며
  시작 시 `max_connections`를 검사한다(#46). 유지보수 job claim이 실행 중 1개를 추가로 쓰며 reserve 안에 든다.
- collab_product는 디버그 helper·병렬 56 스레드에서 짧은 내부 deadline 테스트가 간헐 실패할 수 있다(CI 정상).
  테스트 harness 수정과 용량 작업에서 함께 다룬다.
- 실제 OS IME는 Linux X11 IBus hangul 2벌식·Chromium의 실제 XTEST witness로 수락했다(§2). Windows·macOS·모바일·특정 기기 입력은
  그 witness 범위 밖의 한계로 기록한다(새 필수 조건 아님). 합성 이벤트 성공을 IME 검증으로 표시하지 않는다.
- 제한기는 프로세스 로컬·직접 socket IP 기준이며 신뢰 프록시·분산 제한은 없다.
- 협업 receipt/이벤트/감사 누적은 원본과 같이 보존 정책이 없다. 첨부 중단 업로드 정리는 #63·#65로 수락했다.
- 전역 보안 헤더(원본 nosecone CSP·Referrer-Policy 등)는 #81로 수락했다(공유 응답의 개별 CSP·no-referrer 유지).
- 2026-09-26 감사의 identity link issuer·link tx 세션 재검증·ENCRYPTION_KEYS 복구 검증·`--verify-storage` preview·
  document-extract child env 상속은 #90으로 수정·수락했다.
- 가져오기 POST는 요청당 최대 수백 MiB를 버퍼링하며 프로세스 전체 동시 수 상한이 없다(원본도 버퍼링). Node convert
  helper는 #101·#103·#106·#107로 Rust child로 대체돼 제품 이미지에서 제거됐다.
- collab_product `collab_empty_byte_update_is_rejected`가 #66 CI에서 1회 auth 전 close로 실패했다(로컬 7회
  재현 실패, 재실행 성공). #67로 close code를 남기며 재발 시 원인을 닫는다.
- 컨테이너 AppArmor docker-default 환경(예: GitHub runner)은 helper의 `oom_score_adj=1000` 쓰기를 거부한다. helper는 그대로 시작하고 서버가 1회 경고를 남기며, 이때 cgroup OOM이 서버 대신 helper를 고른다는 보장은 없다(컨테이너 mem_limit·helper별 AS/RSS 한도가 상한). #46
- 검색 색인 소비자는 Meili 단건 쓰기마다 작업 완료를 기다려(측정 1.7–2.6 s) 직렬 처리량이 약 0.5 event/s다. 항상
  수렴하지만 부하 시 색인이 지연된다. 배치/비동기 대기 개선이 후속이다.
- 초대·공유·ICS 경로 토큰과 OIDC code/state가 요청 trace span의 원시 URI로 debug 로그에 남던 문제는 #178로 수정·수락했다
  (span에는 method·`MatchedPath` 템플릿 또는 고정 fallback·version만 기록). 임의 애플리케이션 로그 전체 감사는 아니다.
- TS 데이터 이전은 사용자 확인으로 범위 밖이다(§2).

## 6. 재개

1. `AGENTS.md` → `.agents/environment.md` → 이 문서 → Orca `worker-list --run run_b01d432a9dee` →
   `git worktree list`·각 worktree `git status` → 열린 PR·main CI 순으로 실제 상태를 확인한다.
2. 진행 중 worktree의 미수락 커밋을 보존하고 같은 작업을 중복 배정하지 않는다.
3. 로컬 DB 검사: `scripts/start-test-postgres.sh cargo test --locked --offline --no-fail-fast
   --features db-tests --test <suite>`. 서버 실행 전 `fvoci-migrate` → `fvoci-migrate --grant-app-role <role>`.
4. 설치 확인: `scripts/install-smoke.sh`(RUNNING.md "Container install").
5. 다음 기능은 위 대응표의 미착수·부분 행에서 의존성이 준비된 사용자 흐름을 먼저 고른다.
