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
최신 수락 제품 main은 아래 `e600047c1aef043357061f00fe735a3ce0d9e4ea` 표(#239 merge, 2026-09-29)이며, 그 사이 anchor는 당시 관측으로 보존한다. 열린 제품·CI PR은 아래에
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

2026-09-28 KST 추가 대조 지점(수락 제품 main): `95160cddaffd8961b0e9974e0cedc2f6bd498e9f`(#207 merge). 기존 anchor와 관측은 유지한다.
아래 PR은 코디네이터(Fable 5.1 medium)가 고정 HEAD의 실제 check 성공과 별도 Opus 5.5 medium 독립 검토 ACCEPT를 확인하고
`--match-head-commit`으로 머지했다. 검토 근거는 `/home/kinesis/orca/fvoci-evidence/`의 각 `opus-*-review.md`, 머지 순서·CI 관측은
`coordinator-progress-2026-09-28-fable.md`에 있다. 각 merge의 main push CI는 PR 수락 근거와 구분하며 이 표로 통과를 주장하지 않는다.

| PR | 고정 HEAD | merge | 수락 범위와 한계 |
| --- | --- | --- | --- |
| [#196](https://github.com/AISFlow/fvoci/pull/196) | `ec3da98d` | `8007f46d2693312fb27edf5651cd4b49b667953a` | Push 구독 전 Service Worker 활성화 대기(9회차 `no active Service Worker` 결함 수정). 코드 검토 ACCEPT(`opus-push-worker-activation-review.md`)와 실제 제공자 headed witness 1회 ACCEPT(`opus-push-real-witness-review.md`: Chrome for Testing 153·Linux Xvfb·FCM, 구독·발송·SW 수신·로그아웃 해제). 다른 브라우저·제공자는 아니다 |
| [#197](https://github.com/AISFlow/fvoci/pull/197) | `5204aae2` | `9d46d700d23e629a565c8eaf0a47701be0baf089` | 문서: `3919a326` 수락 범위 정리 |
| [#189](https://github.com/AISFlow/fvoci/pull/189) | `cd70a22d` | `9e15d50e44a74c9230fa10f7a6083527e1543fcb` | HWP/HWPX 첨부 편집과 인가된 사본 저장(원본 bytes 불변, 새 첨부). 통합 composition 검토 ACCEPT(`opus-hwp-final-push-composition-review.md`). 한컴 오피스 편집 동등성은 주장하지 않는다 |
| [#198](https://github.com/AISFlow/fvoci/pull/198) | `5116c0b7` | `80b9d6bdae51c459abdde53e90a5cc04f1a25511` | 문서: Fable 코디네이터 역할 인계 |
| [#200](https://github.com/AISFlow/fvoci/pull/200) | `b7fc69b7` | `bca6adb2cc59052b2e7357e68aecd0200007c626` | 컬렉션 query의 wiki 행 권한 일괄 조회(#76 S4 N+1 해소) |
| [#199](https://github.com/AISFlow/fvoci/pull/199) | `d530e0a5` | `1cf86c81646477c2bcc5341b65060c935309581c` | 유지보수 job claim의 unlock 확인 후 연결 종료, migration 041(누락된 outbox cursor 복구) |
| [#201](https://github.com/AISFlow/fvoci/pull/201) | `22852712` | `0cb71ed69611c790481adaddad3d67ad28969d04` | 워크스페이스 이벤트 로그 API와 설정 활동 영역 |
| [#202](https://github.com/AISFlow/fvoci/pull/202) | `1d963c77` | `5fcfef5980067aedaeec4b1b32f5900424c286e7` | `/health`·`/ready`·`/metrics` probe와 healthcheck CLI(검토 BLOCK→delta ACCEPT) |
| [#203](https://github.com/AISFlow/fvoci/pull/203) | `c0a1b223` | `724869ed810194df0b58869244ea5e34050aba67` | HWP 편집 E2E: history back 전 viewer unmount 대기(테스트 경합 원인 수정, 단언·timeout 불변) |
| [#204](https://github.com/AISFlow/fvoci/pull/204) | `4d98b636` | `144e41da0c6ae942d6f81865c42e49d0909c4c32` | `fvoci secrets audit/rotate`를 `fvoci-migrate --secrets-audit/--secrets-rotate`로 이식, migration 042 |
| [#205](https://github.com/AISFlow/fvoci/pull/205) | `e92c42cf` | `6d9dc629d3dee6115d920b4be6b11dfca7121eb1` | `robots.txt`와 세션 전용 `/api/docs`(vendored Swagger UI, 정적 자산) |
| [#208](https://github.com/AISFlow/fvoci/pull/208) | `0294ec83` | `57c6f07afb924f20dcde5d88a9f29bd291b19d5e` | migration 043(`events_workspace_relay_idx`)과 outbox lag 함수, probe·compose healthcheck 후속 |
| [#209](https://github.com/AISFlow/fvoci/pull/209) | `7fc9e163` | `21cf73a46b65a612de0d14a5673e14c4ec445c4f` | outbox 복구 테스트 전 pool 완전 종료(PG16 "client backends remain" 경합, 테스트 전용) |
| [#206](https://github.com/AISFlow/fvoci/pull/206) | `ae1fac46` | `1101e21b358bfa03faed46209b846c374faeebf5` | tag 기반 0.x pre-release workflow(native amd64/arm64 image, digest smoke 후 tag·release). 첫 tag는 아직 없다 |
| [#210](https://github.com/AISFlow/fvoci/pull/210) | `466b9413` | `7d044977fb030efedb31cbc754eae86e22612bb6` | PPTX Worker 시간 제한 테스트 전 warm-up(테스트 전용) |
| [#211](https://github.com/AISFlow/fvoci/pull/211) | `615d8d71` | `cde1ef83e8b765f774c251958d36e39e1677639b` | 태스크 목록 load-more가 stream 무효화와 겹칠 때 유실 방지 |
| [#212](https://github.com/AISFlow/fvoci/pull/212) | `32b4d068` | `9266b315e19a8dbb3ffa5f7a1bc9b11b9eb852c3` | 컬렉션 board load-more의 같은 경합 수정 |
| [#213](https://github.com/AISFlow/fvoci/pull/213) | `23ff45c1` | `9903998ccff0e4d6b95fead6368664b666fc4a2a` | 규칙 정정: 기본 설치 준비는 앱 컨테이너 시작 절차, 제한된 서버 경계 유지(`opus-pr213-guidance-review.md`) |
| [#214](https://github.com/AISFlow/fvoci/pull/214) | `4226eec5` | `284baebd5221f4b5495a4693128da3ec7471ac84` | opt-in 사용자 체감 성능 기준선 도구(CI 미연결). 결과는 `opus-perf-baseline.md`(소스 빌드 `1101e21b`, 이미지 측정 아님) |
| [#207](https://github.com/AISFlow/fvoci/pull/207) | `93620c7f` | `95160cddaffd8961b0e9974e0cedc2f6bd498e9f` | 단일 명령 사용자 설치: `compose.user.yml`+`.env`, `fvoci` 컨테이너가 root로 준비 후 uid 1000 서버 exec, 5개 secret 파일(root 0400), init 서비스 없음. 검토 F9 BLOCK→delta ACCEPT(`opus-install-simplify-review.md`); F12·F13은 비차단 후속 |

2026-09-29 KST 추가 대조 지점(수락 제품 main): `1367b05c1c431fa98be653784d66fdfb69d8a3bf`(#225 merge). v0.1.0 tag(`57497e2f`) 이후 수락분이다.
#220·#221·#222·#226은 Fable 코디네이터가 별도 Opus 검토 후 머지했다(검토 근거는 `/home/kinesis/orca/fvoci-evidence/`). #228·#227·#225는 Claude Code Opus 5.5 코디네이터가 구현과 다른 컨텍스트의 내장 agent
독립 검토(전체+delta)와 고정 HEAD의 원격 CI 전부 성공을 확인하고 `--match-head-commit`으로 머지했다.

| PR | 고정 HEAD | merge | 수락 범위와 한계 |
| --- | --- | --- | --- |
| [#221](https://github.com/AISFlow/fvoci/pull/221) | `4d007602` | `7c4c1c9449ec255fbff09f05c8807766f68f0db6` | 첨부 viewer 첫 표시 지연 단축, admin·Gantt·첨부·legal 페이지 지연 로드. 이후 CI에서 첫 로드 빈 화면 간헐 관측(원인 미확정, 진단 수집 #229) |
| [#220](https://github.com/AISFlow/fvoci/pull/220) | `2ada34bd` | `e06fa8dfc1896bec4cf43ba6f8cf217f15d55e17` | (독립 검토 ACCEPT는 `ea5dfe27`; 이후 N1–N3 반영 `c8eb6645`·테스트 `2ada34bd`는 Fable 기록) `fvoci-migrate --outbox-reset`(진단·apply). 사용자 설치용 절차 문서 없음 |
| [#222](https://github.com/AISFlow/fvoci/pull/222) | `c6843b26` | `feeee159701cd568d07bc48e985228b3eb2d99b0` | (검토 ACCEPT는 `fdc4c323`, 이후 delta는 Fable 기록) Prometheus 1단계: NaN-on-failure 게이지, RSS·예산 지표, `compose.metrics.yml`(release 파일 아님) |
| [#226](https://github.com/AISFlow/fvoci/pull/226) | `f4c8b8e5` | `a53074f74046af6e4e3c1e9e255fd5d23f29dd8b` | (검토 ACCEPT는 `de340d11`, 조건부 문구 수정 반영 `f4c8b8e5`) 문서: 2026-09-29 인계 체크포인트 |
| [#228](https://github.com/AISFlow/fvoci/pull/228) | `66464864` | `b2ff90ccf5b09d15568d32d2f8a592be72facfd8` | outbox-reset 이전 이벤트/newest 조회의 xid8 text 정렬 결함 수정(digit 경계에서 잘못된 target·external replay floor), 테스트 pool `close_pool`(PG16 CI 실패 원인). 게시 버전 영향 없음 |
| [#227](https://github.com/AISFlow/fvoci/pull/227) | `42b9e56a` | `df0b5921fe88298e71143799fd2dbf50b955311b` | 문서: 2026-09-29 역할(Claude Code Opus 5.5 코디네이터·내장 workflow) |
| [#225](https://github.com/AISFlow/fvoci/pull/225) | `f5a09f2d` | `1367b05c1c431fa98be653784d66fdfb69d8a3bf` | 협업 room 상한에서 grace 지난 빈 room 회수(lease 원자 등록·동시 회수 재탐색·세션 리비전 보존), 단일 socket bounded backoff, 거부 시 '로드되지 않음+이유'. 성능 수치는 초기 head `4a68ee70` 작성자 측정(소스 빌드·단일 호스트)이며 최종 head·게시 이미지 미측정 |

2026-09-29 KST 추가 대조 지점(수락 제품 main): `e600047c1aef043357061f00fe735a3ce0d9e4ea`(#239 merge). v0.1.0 tag 이후 v0.1.1에 포함된
#229·#223은 위 표에 없어 앞 두 행에 기록하고, 나머지는 v0.1.1 tag(`71d252a6`) 이후 수락분으로 0.2.0 준비 PR(마지막 행) merge의 `v0.2.0` 게시 대상이다.
아래 PR은 Claude Code Opus 5.5 코디네이터가 구현·코디네이터와 다른 컨텍스트의 내장 Plan agent 독립 검토(행마다 "검토")와 고정 HEAD의 원격 CI
성공(각 PR head 39 성공·1 skip, 5개 gate 성공)을 확인하고 기대 HEAD를 지정해 머지했다. `w…` 이름은 코디네이터 세션의 workflow run이다.
마지막 열은 merge 커밋의 main push CI이며 PR 수락 근거와 구분한다.

| PR | 고정 HEAD | merge | 수락 범위와 한계 | merge 커밋 main CI |
| --- | --- | --- | --- | --- |
| [#229](https://github.com/AISFlow/fvoci/pull/229) | `ab67a3ca` | `e5eec53936041aaef3546b18acf234e7ffd4587c` | CI 전용: 실패한 web-e2e group의 redaction된 `server.log`·`browser-summary.txt` 보존·업로드, e2e 서버 `RUST_LOG=info,tower_http=debug`. 로컬 ~1,800회 재현 실패. v0.1.1 포함. 검토: 초기 검토와 delta 검토 2회(run `wv33rdfzh`·`w8rfk0v60`·`wmhb1vxz7`) | 39 성공·1 skip, 5개 gate 성공 |
| [#223](https://github.com/AISFlow/fvoci/pull/223) | `9fa6aa1a` | `71d252a6e593adce2b960cefd60b8abe6171df41` | 0.1.1 release 준비(version·계약·노트·이 문서). `v0.1.1` tag 대상(release [36484451812](https://github.com/AISFlow/fvoci/actions/runs/36484451812)). 검토: run `whp738bne`(delta 범위)·`weevirgfz`·`wy13oxr21` | 48 성공·1 skip, 5개 gate 성공 |
| [#230](https://github.com/AISFlow/fvoci/pull/230) | `9685d6a8` | `e02fe71b28ab2cab6a52ef310d727e72dad8e6f0` | 테스트 저장소 스크립트(PG·MinIO·Meili)가 익명 volume까지 제거(`docker rm -f -v`). 테스트 도구 전용, 제품 영향 없음. 호스트에 이미 쌓인 volume은 건드리지 않음. 검토: 전체·delta(run `weevirgfz`·`wy13oxr21`) | 39 성공·1 skip, 5개 gate 성공 |
| [#234](https://github.com/AISFlow/fvoci/pull/234) | `3ec4240a` | `5c13ee37883493e617ceff1eb47d9413a6d583e9` | 감사 WP2: SSE 접근 검사 단일 트랜잭션·project row lock 없음(SSE-AUTH-07), access stream이 `workspace_member.removed`·`role_changed`·`workspace.deleted`에서 닫힘(SSE-ACCESS-06), settled-horizon cursor(`pg_snapshot_xmin`, SSE-CURSOR-02), 브라우저 EventSource 재개(1 s→30 s jitter backoff)와 컬렉션 stale cursor 복구(http-F3). migration·wire 변경 없음. 64 stream poll 비용 미측정. 검토: 독립 Plan-agent 검토(다른 컨텍스트) | 39 성공·1 skip, 5개 gate 성공 |
| [#235](https://github.com/AISFlow/fvoci/pull/235) | `fae3898e` | `edce73fba7ad144581e79b6bfa3721b805cb6cb8` | 감사 WP6: local 저장소 재개 시 part 재해시 제거(etag sidecar, 조립 시 전체 hash 검사 유지; 1 GiB 재개 CPU ~24 s→12 ms는 작성자 로컬 측정), 계정 export가 저장소 재검사 오류 시 중단, 24 h 넘은 unleased pending markdown-zip import 행 실패 처리, part 정리 오류 로그. S3 불변, migration 없음. 검토: 독립 Plan-agent 검토(다른 컨텍스트) | 39 성공·1 skip, 5개 gate 성공 |
| [#233](https://github.com/AISFlow/fvoci/pull/233) | `59a0925d` | `0a04131c7601202a5852a50ed5c1f70f478eb43c` | 감사 WP4: rate limiter 키별 window·최대 namespace 축출(http-F1), instance-admin 쓰기의 `require_admin_session`(ARX-2, 대기 중 철회된 세션 404·무기록), `restore.sh` 앱 비밀번호 argv 제거(install-F02). 미포함: IPv6 회전·계정별 로그인 제한(정책), `\gexec` `CREATE ROLE … PASSWORD`의 `log_statement>=ddl` 로그 노출(문서화). 검토: 독립 Plan-agent 검토(다른 컨텍스트) | web-ci-gate 실패: `web-browser-shard-4` project-trash-flow 첫 로드 빈 화면(`net::ERR_NETWORK_CHANGED`, #237 이전, run [36488304394](https://github.com/AISFlow/fvoci/actions/runs/36488304394)). 나머지 4개 gate 성공 |
| [#237](https://github.com/AISFlow/fvoci/pull/237) | `8d714a9a` | `933c51c8230f04028a6763fcfe9a854a3f5a051b` | CI 전용: web-e2e group별 netlink 이벤트 기록(`net-events.log`)과 Playwright 전 네트워크 settle 대기(10 s 상한 후 경고, 재시도·timeout 변경 없음). 첫 로드 빈 화면의 완화이며 원인 증명이 아니다(§5). 검토: 독립 Plan-agent 검토(다른 컨텍스트) | 39 성공·1 skip, 5개 gate 성공 |
| [#231](https://github.com/AISFlow/fvoci/pull/231) | `32278fb6` | `ea0e85a84e831b441257d4d33764b425f86e4fc9` | 감사 WP1: 프로젝트 읽기를 `begin_read`(REPEATABLE READ READ ONLY, xid 없음)로 옮겨 project row lock 제거, 수동 리비전 쓰기 fence(ARX-4), 댓글 권한 DB 오류 500(ARX-5), 검색 색인·WIP advisory namespace 분리(ARX-7), task·document backlink 15 s statement timeout. sqlx 0.8.6 BEGIN 취소 누수 대응 앱 pool acquire 검사(`src/db/pool.rs`, `tests/pool_release_integration.rs`, §5). 검토: 전체·delta ACCEPT_WITH_NITS → pool delta REQUEST_CHANGES → 최종 delta ACCEPT(`32278fb6`) | 39 성공·1 skip, 5개 gate 성공 |
| [#232](https://github.com/AISFlow/fvoci/pull/232) | `31d5fc83` | `086f2b4d398a7b460612e36274f0c11ddedfc70f` | 감사 WP3: External 전달(mail·search-index·설정된 GitHub)의 종료·lease 예산 중단(F8), 실패 이력 event 단독 전달(F1), batch 후 lease 갱신·mark 후 진행(F6), 수신자 단위 메일(F2, 수락 수신자 메모리 64 event), digest keyset paging·15분 예산·실패 streak 중단(F4), `--recover-outbox` 재생 window mark 보존(F5). 잔여: 수락 수신자 목록은 메모리만, digest streak 한계(§5). migration 없음. 검토: 독립 Plan-agent 검토(다른 컨텍스트) | rust-ci-gate 실패: `postgres-pg16-b` `collections_integration` `wiki_collection_can_edit_uses_one_set_based_permission_lookup` 3행/21행 문 목록 불일치(#231 acquire 검사 문 1개 차이, run [36498519146](https://github.com/AISFlow/fvoci/actions/runs/36498519146); 테스트 전제 결함, #239로 수정, §5). 나머지 4개 gate 성공 |
| [#238](https://github.com/AISFlow/fvoci/pull/238) | `7a1529dd` | `1b8666a5e84aad763fddef3f46b8ca49705e8a9d` | 감사 WP5: 서버 `PR_SET_DUMPABLE 0`(실패 시 기동 거부, install-F01), helper 자체 `oom_score_adj=1000`과 부모 확인, helper PDEATHSIG(collab-F10), 지연·복구 가능한 engine bridge(collab F1/F4, room open당 helper 1개), helper slot 포화 1013(이전 1011), `collab_product` 예약 교착 테스트 수정. migration 없음. 미실행: AppArmor docker-default 측정. 잔여: `RUNNING.md` perf/seccomp 문구 nit(§5). 검토: 독립 Plan-agent 검토(다른 컨텍스트) | 39 성공·1 skip, 5개 gate 성공 |
| [#236](https://github.com/AISFlow/fvoci/pull/236) | `5866007b` | `31459b3c9228a0ea4f27848bfee5536ced116b3f` | identity·공개 endpoint 강화(0.2.0 minor 사유): workspace SSO별 callback `/api/v1/auth/sso/{workspace_id}/callback`(OIDC-1 IdP mix-up), 초대 OIDC start same-origin POST(INV-1, 403 `origin_mismatch`), 초대 수락 login 한도·IP당 60/5분(INV-2), TOTP 최신 step claim(MFA F3), 동시 `--secrets-rotate` 검증(MFA F4), migration 044 이메일 변경 시 `auth_generation` 증가(MAG-1), wiki 공유 링크 생성 시 하위 트리 View 요구(Share F1 A), 개인 workspace SSO 저장 409·기존 행 비활성, 서버 제공 `redirectUri`, link·SSO slug의 script 시작(Chromium CSP). 사용자 결정 대기: ACC-1, Share B. 잔여: team workspace 관리자의 GET start login CSRF, 검토 nit(§5). Chromium 153 1회 실행만(브라우저 테스트 미커밋). 검토: 독립 Plan-agent 검토(다른 컨텍스트) | 39 성공·1 skip, 5개 gate 성공 |
| [#239](https://github.com/AISFlow/fvoci/pull/239) | `c55bee4d` | `e600047c1aef043357061f00fe735a3ce0d9e4ea` | 테스트 전용: `collections_integration` `wiki_collection_can_edit_uses_one_set_based_permission_lookup`의 문 목록 비교에서 #231 acquire 검사 문을 제외(sqlx는 재사용 idle 연결에만 `before_acquire`를 실행하므로 검사 수가 pool 상태에 따른다). `086f2b4d` main의 `postgres-pg16-b` 실패(run 36498519146) 수정. 제품 변경 없음. 검토: 독립 Plan-agent 검토(다른 컨텍스트) ACCEPT_WITH_NITS(run `wq2qspket`) | 39 성공·1 skip, 5개 gate 성공 |
| #`<TBD>` | `<TBD>` | `<TBD>` | 0.2.0 release 준비: `Cargo.toml`·`Cargo.lock` 0.2.0, `apps/web/openapi.json`, `scripts/release-notes-template.md`(`notes-for: 0.2.0`), 이 문서. merge 커밋이 `v0.2.0` tag 대상. 검토: `<TBD>` | `<TBD>` |

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
최신 실행·소유권·검증 SHA·인계 포인터는 `/home/kinesis/orca/fvoci-evidence/coordinator-progress-2026-09-28-fable.md`에 둔다
(이전 `coordinator-handoff-2026-09-28-0145-utc-linux.md`는 그 시점 이력으로 유효).
2026-09-28 인계 관측: #197 문서·#196 Push·#189 HWP가 수락되어 main은
`9e15d50e44a74c9230fa10f7a6083527e1543fcb`다. 각 PR의 수락 근거와 최신 main push CI는
구분한다. 당시 후보였던 DB claim·outbox migration 수정 #199는 이후 위 `95160cdd` 표로 수락됐다.
아래 이전 상태표는 작성 시점의 관측이며 새 인계에서 완료된 PR을 중복 배정하지 않는다.

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
- 이전 C였던 HWP 편집·인가된 사본 저장(#189, `cd70a22d`)과 Web Push 활성화 수정(#196, `ec3da98d`)은 위 `95160cdd` 표로 수락됐다.
  Web Push 실제 제공자 F는 Chrome for Testing 153·Linux·FCM 1회 witness로 수락했고, 다른 브라우저·제공자는 그 범위 밖의 한계다.
- 0.1.0 trial release 준비(2026-09-28): 발행 노트는 `scripts/release-notes-template.md`, 절차는 `docs/RELEASING.md`.
  2026-09-28: `v0.1.0` tag는 `57497e2f`(#215 merge)에 있다. release 실행 36416132900(main `54dcfc86`, workflow_dispatch)은
  index `sha256:638aad92f5b48f9e48c929552c3dc567548be1b73c1ff1dfbc31e9e7ac4e1e11`을 빌드했고 GHCR 익명 manifest 조회는 성공했으나,
  digest smoke가 테스트 클라이언트 cookie 결함으로 실패해 실행은 failure였다(#218로 수정). 이후 release 실행 36433264742가
  `v0.1.0`을 게시했다(index `sha256:02380fef1b906eb0be6de6bdbd94f338595ba62ae26b7ef301319d57fad07cfe`, smoke 도구 `06b039e4`).
- 0.1.1·0.2.0 trial release: `v0.1.1` tag는 `71d252a6e593adce2b960cefd60b8abe6171df41`(#223 merge)에 있고 release 실행
  [36484451812](https://github.com/AISFlow/fvoci/actions/runs/36484451812)(tag push, 제품·smoke 도구 SHA 동일)이 게시했다(index
  `sha256:419529858229c6792612ee5bda38521ccceac366cec08aee65b4537edb484223`, `:0.1` 동일). `v0.2.0` tag는 `<TBD>`(0.2.0 준비 PR merge)이며
  release 실행 `<TBD>`이 게시한다(index `<TBD>`). 0.2.0은 migration 044와 workspace SSO redirect URI 변경 때문에 minor다.
- 사용자 설치 설계(#207): `infra/rust/compose.user.yml`+`compose.user.env.example`(release의 `compose.yml`·`env.example`),
  서비스 `fvoci`·`postgres`·`meilisearch`, 준비(설정 검사·준비 확인·앱 역할·migration·grant·검색 키)는 `fvoci` 컨테이너의
  `fvoci-migrate --start`가 root로 수행한 뒤 uid 1000 `fvoci-server`를 exec한다. `infra/rust/compose.yml`(init 서비스)은
  개발·소스 빌드 경로다. 설계 근거 `opus-install-simplify.md`, 검토 `opus-install-simplify-review.md`.
- 성능 기준선(#214 도구, 측정 `opus-perf-baseline.md`, 검토 `opus-perf-baseline-review.md`): 소스 빌드 `1101e21b`, 단일 호스트.
  사용자 체감 상위 병목은 협업 room 상한 도달 시 새 본문 최대 ~30 s 정지와 빠른 재연결 반복, 첨부 viewer의 ~300 ms 유휴 구간,
  태스크 메타 원격 반영의 SSE 750 ms poll이다. 첨부 viewer 첫 표시는 #221, room 상한 동작은 #225로 개선했다(최종 head·게시
  이미지 재측정 없음). SSE 750 ms poll은 설계 결정 대기다.
- 실제 외부 IdP·인증 앱 스캔 F는 사용자 환경(계정·기기)이 필요한 실행 증거이며 코드 정책 선택 문제가 아니다.
- 분류 근거·행별 종료 조건: `/home/kinesis/orca/fvoci-evidence/opus-decision-status-cleanup.md`.
- 문서 변환의 정상 Rust 경로는 수락됐으며 Node 구현 복원은 필요 없다. 개발·CI의 Node/Python과 독립 reader는
  제품 런타임과 별개다. 필수 백업·복원 자체 검증은 Rust이며 pg_dump/pg_restore·얇은 실행 스크립트는 유지한다.
- migration 037–043은 main에 수락됐고 044는 #236으로 수락됐다. 번호를 다시 배정하지 않는다.
- 잔여 기능·검증: #149 S3 전송 E, 인증 앱 스캔·#77 외부 IdP F. `fvoci outbox-reset`은 #220(독립 검토 ACCEPT `ea5dfe27`, 이후
  `c8eb6645`·`2ada34bd`는 Fable 기록)과 #228(target 정렬 결함 수정, 독립 검토 ACCEPT)로 수락됐고, `v0.1.0`은 release 36433264742로 게시됐다. 추가 DB는 원본도 제품 백엔드가 아닌 G로 종료(§3). wiki 컬렉션 권한 N+1(#76 S4)은 #200으로,
  HWP 편집·사본 저장은 #189로, 실제 Web Push 제공자 1회 witness는 #196 범위로 수락됐다(HWP 보기 #187·PPTX #188·DOCX #180·XLSX #186 수락). 수락(`3919a326`): S3 이미지 업그레이드·versioned rollback 로컬 S3 호환 silo x64 실행(#190; 실제 클라우드
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

상태: 수락(main 반영·검증) / 부분(일부 경로 수락) / 진행 / 미착수 / G(원본도 제품으로 제공하지 않은 범위, 새 범위로 열지 않음). "남은 차이"는 항목별로 진행·미착수·미검증·
후속(수락 범위의 비차단 개선)으로 구분하며, 전체 포팅 완료 전
해결하거나 사용자가 승인한 차이로 기록해야 하는 항목이다.
부분 행의 잔여 분류(기준 main `6ba876503e2a8340c0c21f8f110d2ec14f67f8b3`, 2026-09-27): A 오래된 표기(이미 수락) ·
B 명확한 구현·문서 작업 · C 수락 대기(열린 PR·진행 워커) · D 결함 · E 사용자 정책 결정 필요 · F 외부 환경 검증 필요(생략 불가) ·
G 선택·미승인·원본 부재(새 요구 없이 만들지 않음). 각 행의 "종료:"가 그 행을 닫는 조건이다.
2026-09-28 행 정리(`fvoci-evidence/grok-remaining-rows-closure.md`, 원본 `39379526` 대 main `54dcfc86`): 남은 항목은
수락(main) / B 잔여 구현 / E 사용자 결정 / F 외부 검증 / G·후속(S4 포함, 비차단 개선) / 첫 release 절차 미실행으로 나눠 적는다.

| 기능 | 원본 근거 | 보존할 불변식 | 상태 | 증거 | 남은 차이 |
| --- | --- | --- | --- | --- | --- |
| 설치·로그인·세션·프로필 | identity/routes.ts, core/auth.ts | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | 부분 | #1, #34, #53, #69, #72, #233, #235, #236 | 수락: 비밀번호 재설정(#53), 탈퇴·익명화·비밀번호/이메일 변경·magic link·export(#69), 설정 기반 비밀번호 최소 길이(#72), TOTP MFA·OIDC·workspace SSO(#77), MFA 등록 QR(SVG)·수동 키 UI(#84). 수락(0.2.0): workspace SSO별 callback `/api/v1/auth/sso/{workspace_id}/callback`(IdP mix-up 차단, 서버 제공 `redirectUri`), 개인 workspace SSO 저장 409와 기존 행 비활성, TOTP 최신 step claim·동시 `--secrets-rotate` 중 검증, 이메일 변경 시 `auth_generation` 증가(migration 044; 이전 메일 링크·MFA challenge 무효, 세션 유지), 계정 연결·SSO slug의 script 시작(#236); rate limiter 키별 window·최대 namespace 축출(#233); 계정 export가 저장소 재검사 오류 시 중단(#235). OIDC 변경은 로컬 test provider와 Chromium 153 미커밋 1회 실행으로만 확인했다. 잔여 구현 없음(원본 인증·세션·MFA QR·OIDC 코드 경로는 모두 수락). F: 실제 인증 앱 스캔 witness, #77 실제 외부 IdP 실행 witness(브라우저 구조 검사와 구분; 사용자 환경 필요, 코드 정책 선택 아님). 사용자 결정 대기(§5): team workspace 관리자의 GET start login CSRF 수용 여부. 종료: F 실행 증거 또는 사용자가 F를 한계로 승인, 위 사용자 결정 반영 |
| 워크스페이스 | domains/workspaces | 현재 역할·철회 경합·RLS·풀 컨텍스트 | 수락 | #4, #39, #56, #61, #63, #110, #134, #201, #208, #234 | 수락: counts·owner 전용 삭제(#56), 30일 purge 실행기(#61), S3 저장소 purge(#63), 원본 사용권별 workspace/guest/storage quota(#110), workspace ZIP 내보내기(#134), 워크스페이스 이벤트 로그 API·설정 활동 영역(#201; 조회 index migration 043 #208). 수락(0.2.0): workspace access stream이 본인의 `workspace_member.removed`·`role_changed`와 `workspace.deleted`(trash)에서 닫히고 tick마다 membership을 확인(#234). A: 남은 차이 없음 |
| 멤버·초대 | invitation.ts, quota.ts, consent.ts | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | 수락 | #21, #53, #69, #72, #77, #84, #236 | 수락: 초대 메일(#53), 초대 수락의 legal consent 428·`defaults.user` 적용(#72), 탈퇴·관리자 삭제 시 보낸 pending 초대 정리(#69·#84), 초대 수락 시 MFA challenge·OIDC 초대 수락(#77). 수락(0.2.0): 초대 OIDC 시작을 same-origin `POST /api/v1/auth/oidc/{provider}/start`로(다른 origin·`Origin` 없음 403 `origin_mismatch`, `GET` start의 `invitation`·`consents` 400), 기존 계정 초대 수락 비밀번호의 login 한도 적용과 IP당 60/5분 수락 한도(#236). 사용자 결정 대기(§5): ACC-1(OIDC로 만드는 초대 계정에 초대 이메일 일치·검증 요구; 원본 `39379526`도 요구하지 않음). A: 알림/사용자 기본값·계정 삭제 정리·E2E SQL fixture(없음). G: pending 목록/철회 API(원본 contracts·UI에 없음) |
| 그룹·권한 통합 | policies.ts effectivePermission, project/document_members(user XOR group) | 리소스별 단일 권한 함수 | 수락 | #23, #39, #50 | 의도적 차이(§4): 프로젝트 문서 그룹 route는 원본에서 항상 404인 표면이라 등록하지 않는다. 후속: collab 프레임당 권한 재조회 축소·collab_delivery의 그룹 join 사본·설정 UI `canManage` DTO |
| 프로젝트 | domains/projects | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | 수락 | #13, #39, #52, #76, #78, #80, #83, #161, #231 | 수락: 프로젝트 문서 협업·trash/restore/sort·archive·30일 purge(#78), 프로젝트 문서 body/children/ancestors/backlinks/duplicate(#83), collection/view 복제(#52·#76), 프로젝트 문서 첨부(#80)·태그 route. 기존 B 표기 정정(2026-09-27): 프로젝트 문서 리비전 API·UI·forward 복원은 #161 수락, 프로젝트 그룹 route(`src/http/routes/groups.rs`)·UI(`project-groups.tsx`, `workspace-groups-flow` E2E)는 #39에서 이미 수락(A). 수락(0.2.0): 프로젝트·멤버·workflow·group grant·label·milestone·프로젝트 문서 읽기가 project row lock·xid 없이 `begin_read`(REPEATABLE READ READ ONLY) 한 snapshot으로 세션·권한·데이터를 읽는다(#231; 쓰기는 기존 lock 순서 유지). A: 남은 차이 없음 |
| 태스크 | domains/tasks, core/task.ts, workflow.ts | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | 수락 | #13, #19, #26, #38, #40, #47, #60, #142, #163, #168, #182, #211, #212, #231, #234 | 수락: activity feed(#60), 본문 협업·block patch·수동 리비전(#104), origin UI/API(#117), archive 전 본문 영속화(#121). Gantt 조회·편집 UI(#130)는 수락. 수락(기준 이후): board 그룹 paging·drag-and-drop(#142), 태스크 메타 변경 SSE 전달(#163), 태스크 목록·layout의 `dueBefore`·due 정렬/cursor actor 시간대와 잘못된 저장 시간대의 UTC fallback(#168), PATCH committed 응답의 상세 cache 반영으로 다음 편집 유실 방지(#182), 목록·컬렉션 board load-more와 stream 무효화 경합(#211·#212). 수락(0.2.0): 태스크 읽기(상세·목록·의존·activity·time entry·backlink·parent)의 row lock·xid 제거, backlink 15 s statement timeout, task-status WIP advisory namespace 분리(#231); stream 접근 검사 단일 트랜잭션·settled-horizon cursor·브라우저 EventSource 재개(1 s→30 s jitter backoff)(#234; 64 stream poll 비용 미측정). 한계: 다른 브라우저의 메타 반영은 SSE 750 ms poll 지연(§5). A: 남은 차이 없음(컬렉션 query의 저장 시간대 검증 D는 공유·컬렉션 행) |
| 일정·ICS·휴일 | routes.ts ics/holidays | 일정 의미 | 수락 | #49, #76 | 수락: 휴일·ICS 피드(담당 태스크, #49), 저장 calendar view 기반 ICS 분기(#76). A: 남은 차이 없음 |
| 위키 문서 | domains/documents, core/document.ts | 현재 문서 권한·트리 잠금 순서 | 수락 | #5, #23, #39, #66, #70, #78, #83, #231, #235 | 수락: 가져오기·내보내기(#66), 공유 링크(#70), trash 30일 purge(#78), body/block patch/children/backlinks/duplicate/flat(#83). 수락: office·Notion 가져오기(#85), Rust 변환·내보내기/doctor·Node 없는 제품 이미지(#101·#103·#106·#107). 수락: 가져오기 복구 보상(#112), 문서·태스크 템플릿(#118). 수락(0.2.0): 문서 backlink 15 s statement timeout(#231), 24 h 넘은 unleased pending markdown-zip 가져오기 행을 daily sweep이 실패 처리(#235). A: 남은 차이 없음 |
| 리비전 | documents/revisions.ts, core/revision.ts, collab applyRestore | 복원은 room actor의 forward system update, durable 후 broadcast | 수락 | #25, #104, #123, #128, #151, #161, #194, #231 | 수동·session·scheduled 리비전과 자동 보존 개수 제한은 수락. A: 리비전 route의 `document_permission` 인가. 기존 D 의심(writer-stale 후 연결 없는 room의 수동 캡처가 오래된 본문 저장): #151에서 실제 앱 역할 PG·WebSocket 재현 후 수정·수락. 프로젝트 문서 리비전(#161) 수락. 수락(0.2.0): 수동 리비전 생성이 membership advisory lock·세션 재검사 후 project `FOR SHARE`를 잡아 대기 중 철회된 세션·정지된 PAT 소유자·제거된 멤버를 404로 거부(ARX-4), 리비전 읽기 row lock 제거(#231). A: 남은 차이 없음 |
| 댓글 | comments/routes.ts, core/comment.ts | 문서 XOR 태스크·부모 활성·권한 | 수락 | #28, #47, #58, #146, #231 | 수락: 그룹 멘션·프로젝트 문서 댓글(#58; 그룹 멘션은 원본 snapshot과 달리 전달 시점 재전개). 수락(기준 이후): 이중 DELETE 중복 이벤트·resolve 경합·동시 trash된 태스크 댓글(#146). 수락(0.2.0): 댓글 권한 검사의 DB 오류는 404 대신 기록된 500(ARX-5), 댓글 목록 읽기 row lock 제거(#231). A: 남은 차이 없음 |
| 협업 | domains/collab, React/Tiptap | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | 수락 | #6, #7, #18, #24, #27, #39, #46, #114, #131, #194, #238 | 수락: room 용량(기본 30, 64 검증; §2), helper SIGKILL 후 복구(#131). A(오래된 표기 정정): actor panic/rejoin(1011 종료·자원 해제·durable 상태 재적재 후 successor 1개)은 `collab_lifecycle` 20개의 panic/rejoin 명명 테스트가 #114 x64/ARM64 협업 실행에서 통과해 수락됐다. 제품 이미지·compose는 `FVOCI_COLLAB_ENGINE`을 기본 설정하고 단독 서버는 그 env가 필요하다(정책·코드 변경 없음). 수락: lease 축출 후 늦은 Leave 무시·backpressure 뒤 session 리비전 보존(#194). 수락(기존 F 종료, 별도 검토 ACCEPT): 실제 OS IME witness(Linux X11 IBus hangul 2벌식·Chromium 153 실제 XTEST 입력, preedit 중 원격 갱신·Backspace/undo·persist ACK·graceful 재시작 후 재열기; §2). 한계(새 필수 조건 아님): Windows·macOS·모바일 IME, 같은 node 삽입 경합은 이 witness 범위 밖이다. 수락(0.2.0): 지연·복구 가능한 engine bridge(room open당 helper 1개, spawn 실패 후 다음 recycle에서 복구), helper slot 포화 시 1013(이전 1011), helper PDEATHSIG, helper 자체 `oom_score_adj=1000`과 부모 확인(#238; AppArmor docker-default 재측정 없음, §5). A: 남은 차이 없음 |
| 첨부 | domains/attachments, packages/storage | 부모 권한·원본 bytes·원자 완료·취소 | 부분 | #10, #39, #58, #63, #65, #80, #148, #176, #180, #186, #187, #188, #189, #203, #210, #235 | 수락: viewer route(#58), S3 프록시·중단 업로드 GC(#63, #65), 태스크·프로젝트 문서 부모·DELETE·quota·이미지 preview·저장 추출문 preview-html(#80). 수락: `--verify-storage` preview 객체·크기 확인(#90). E: S3 전송 정책의 사용자 직접 선택 확인([#149](https://github.com/AISFlow/fvoci/issues/149)) — 원본은 S3 presigned part PUT·GET이 동작하고(`packages/storage/src/s3.ts`), Rust는 수락된 API 프록시를 쓰며 `S3_PUBLIC_ENDPOINT`는 미사용이다. 확인 전 API 프록시 유지, presigned를 기본 잔여 구현으로 열지 않는다. 잔여 구현·수락 대기(C) 없음. 수락: 미추출 파일의 preview-html 즉석 격리 parse(#171), PDF layout viewer(#174), 공개 공유 첨부 deep-link UI(#176), DOCX layout viewer·세션 `?chunk` 보조 표시·ZIP 예산/숨은 entry 격리(#180; Word desktop reflow 동등성 아님). 수락: XLSX layout viewer(#186; Excel desktop 동등성·차트·편집 아님). 수락: HWP/HWPX viewer(#187), PPTX viewer(#188; PowerPoint desktop 동등성 아님). 수락: HWP/HWPX 편집·인가된 사본 저장(#189; E2E 경합 원인 수정 #203, PPTX 시간 제한 테스트 #210). 추출 plain text는 layout viewer와 동등하지 않다. 수락(기준 이후): 업로드 완료 재시도·요청 상한 413/524(#148), 다운로드의 scope PAT 허용(#164, 기존 D 해소). 수락(0.2.0): local 저장소 업로드 재개 시 part 재해시 제거(etag sidecar, 조립 시 전체 hash 검사 유지; 1 GiB 재개 CPU ~24 s→12 ms는 작성자 로컬 측정), 저장 완료 재호출의 잔여 part 정리(#235; S3 불변). 종료: E 확인·결정 반영 |
| HWP/HWPX 추출 | 원본 추출 경로, rhwp e8800c8 | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | 수락 | #2, #8, #9, #11, #35 | A(오래된 표기 정정): lease 만료·재시도 결과 게시 경계는 #11의 lease token 게시 fence로 수락됐고, 탈취된 lease의 finish 0행·재시도 소진 `worker_failure`를 실제 PG 회귀가 확인한다(아래 A 근거). G: HWP 썸네일(원본 thumbnail은 이미지 MIME만, 새 요구로 만들지 않음) |
| 검색·색인·AI | domains/search, packages/search | 검색에서도 인가·철회·색인 복구 | 수락 | #29, #30, #35, #48, #57, #58, #82, #83, #159, #170, #200, #231 | 수락: 워크스페이스·전역 검색, 댓글 hit, outbox 색인 배치(#57), 복구 후 rebuild, PAT scope 검색(#83), 의미(벡터) 검색(#82), 첨부 hit의 viewer 이동(#58, search E2E). A: 검색·색인·의미 검색은 수락. 수락(기준 이후): 문서 메뉴의 `features.ai` 소비(#159). 수락(anchor 이후): AI 결과의 live 문서 적용·프로젝트 태스크 생성 UI(#170, 기존 B 해소), 컬렉션 query 권한 일괄 조회(#200, #76 S4 N+1 해소). 수락(0.2.0): 검색 색인 advisory lock의 전용 namespace(#231, ARX-7; 구·신 서버 혼합 시 직렬화되지 않으나 제품은 rolling upgrade를 거부). A: 남은 차이 없음 |
| 알림·outbox·메일·webhook·연동 | domains/notifications, packages/jobs | 커밋 후 전달·중복/재시도 | 수락 | #31, #35, #45, #139, #196, #199, #208, #209, #232 | 수락: 앱 내 알림, 메일·digest(#53, #61), webhook·GitHub App·AI 동작(#74). 수락: Web Push와 로그아웃 시 브라우저 연결 해제(#139), Service Worker 활성화 대기 수정과 실제 제공자 witness 1회(#196; Chrome for Testing 153·Linux·FCM). 수락: claim unlock 확인·누락 outbox cursor 복구 migration 041(#199), outbox lag 함수(#208), 복구 테스트 pool 종료(#209). 수락(0.2.0): External 전달(mail·search-index·설정된 GitHub)의 종료·lease 예산 중단과 batch 후 lease 갱신·mark 후 진행, 실패 이력 event 단독 전달, 수신자 단위 메일(수락 수신자는 메모리 64 event), digest keyset paging·15분 예산·실패 streak 중단(#232). 한계(§5): 수락 수신자 목록은 프로세스 메모리만(재시작 시 재발송 가능), digest streak가 매일 같은 행에서 끝날 수 있음. 한계(새 필수 조건 아님): 다른 브라우저·푸시 제공자, 실제 SMTP·GitHub App 제공자 실행. G: requeue 운영 API/UI(원본도 없음) |
| 공유·즐겨찾기·최근·태그·컬렉션 | 해당 routes | 공유 링크 권한 | 수락 | #70, #72, #76, #84, #138, #142, #162, #167, #168, #234, #236 | 수락: 즐겨찾기·최근·공유 링크·공개 공유 페이지·PDF, 공유 정책(#72), 태그·컬렉션·저장 view(#76), 대화상자 정책·`/s/:token` head meta(#84), 공유 첨부 preview(#80). 수락: `HEAD /s/:token` 보안 헤더·빈 본문(#138). 수락(기준 이후): board 그룹 paging·drag-and-drop(#142), calendar view drag(#162)와 stream 갱신 후 충돌 검사 영구 회귀(#167), 태스크 목록·layout의 `dueBefore` actor 시간대(#168; 컬렉션 query는 이미 actor 시간대). 수락: 컬렉션 저장 시간대 UTC fallback(#172, #168 검토 F1 해소). 수락(0.2.0): wiki 공유 링크 생성 시 노출 하위 트리 전체의 View 요구(#236 Share F1 A, 거부 404; 기존 링크는 재검사하지 않음), 컬렉션 board·panel의 만료 cursor 복구(#234). 사용자 결정 대기(§5): 생성자 권한을 잃은 뒤에도 링크 유지(Share part B, 원본과 동일). 수락: 공개 공유 첨부 deep-link UI(#176) |
| 동의·감사·사용권·관리 | legal, auth.consents, admin.audit, packages/ee | 동의 gate·증거·권한 | 수락 | #72, #84, #159, #165, #205, #233 | 수락: 관리 API·instance settings·법률 문서·동의·428 gate·branding(#72), 관리자 사용자 삭제 예약/취소(#84). 수락: 사용자가 원본 정책 보존을 확정한 사용권·quota(#110), 운영자 정보(#119), security.txt(#120), 브라우저 오픈소스 고지(#122). 수락: settings `embed`(#124)·`attachmentPreview`(#80) 소비, 문서 메뉴 `features.ai` 소비(#159), 서버 메시지 `i18n.overrides` 사용 시점 적용(#165), `robots.txt`와 세션 전용 `/api/docs`(#205). 수락(0.2.0): instance-admin 쓰기(사용자·instance admin·erase와 취소·legal 게시·settings·branding)의 `require_admin_session`으로 대기 중 철회된 세션은 404·무기록(#233, ARX-2). G: 사용권 issuer trust(원본도 비어 있음) |
| 제품 MCP·CLI·백업·복구 | init.ts, backup.ts, doctor.ts, MCP | 프로토콜·복원 | 수락 | #32, #31, #35, #63, #81, #84, #90, #109, #202, #204, #207, #220, #228, #232, #233 | 수락: 컨테이너 설치 백업·복구(outbox cursor 재기준·검색 rebuild 포함), S3는 스크립트 백업 거부·복구 후 `--verify-storage`(#63, branding #84). 수락: 제품 MCP·CLI·doctor(#81), 키 fingerprint·`--verify-secrets`(#90), Rust 백업 manifest/preflight·키 검증 공유(#109). 수락: healthcheck CLI(#202), `fvoci secrets audit/rotate` → `fvoci-migrate --secrets-audit/--secrets-rotate`(migration 042, #204), 사용자 compose의 `backup.sh`/`restore.sh`(#207, 로컬 standalone smoke). 원본 `fvoci` 명령 대응(2026-09-28 대조): 수락 대응 — `bootstrap`(`fvoci-migrate`+`--grant-app-role`), `backup collect/inspect/restore`(`backup.sh`·`--restore-preflight`·`restore.sh`+`--verify-storage/--verify-secrets`), `outbox-recover`(`--recover-outbox`), `doctor`(`--doctor`), `healthcheck`(`fvoci-server healthcheck`), `init`(`--init-env`), `secrets audit/rotate/rotate-vapid`, `search-rebuild`(`--rebuild-search`), MCP stdio. 의도적 차이(단일 서버 in-process) — `all`·`api`·`worker`·`compact`·`thumbnail`·`collab` 분리 프로세스, `healthcheck <role>`, `doctor [mode]`, `reindex`(Redis 큐 대신 DB claim 추출·embed 루프), `backup identity`(호스트 스크립트). dev 전용(제품 아님) — `seed-dev`, `GET /api/v1/dev/mailbox`. 수락: `fvoci outbox-reset` → `fvoci-migrate --outbox-reset`(skip 진단·`--override-reason` cursor-to-processed; `--recover-outbox`로 대체되지 않음, #220; 이전 이벤트/newest 조회가 xid8 text 정렬로 digit 경계에서 잘못된 target을 고르던 결함은 #228로 수정·회귀 테스트). 수락(0.2.0): `restore.sh`가 앱 비밀번호를 docker·psql argv 대신 환경으로 전달(#233; `log_statement`가 `ddl` 이상이면 `CREATE ROLE … PASSWORD`가 PG 로그에 남음, 스크립트에 문서화), `--recover-outbox`가 재생 window의 processed mark를 갱신해 processed GC에서 보존(#232). G·후속(비차단): MCP `--http` transport, `restore.sh`의 `.env` 따옴표 값 해석을 Compose와 맞춤(검토 F12). 개발 Python oracle는 유지. 종료 조건(`outbox-reset` 수락) 충족 |
| 설치·배포 산출물 | infra/app, compose | 비특권 서버 실행·준비 후 제한 역할 서버·helper 포함 | 수락 | #17, #20, #29, #32, #35, #169, #175, #177, #181, #190, #192, #202, #206, #207, #208, #223, #238 | 수락: 운영 TLS/secure cookie 안내와 같은 volume의 이미지 간 업그레이드·init 실패·이전 이미지 복귀 절차 문서(#169), PostgreSQL 16/17 `uuidv7()` 제품 호환(#177), 이미지 간 업그레이드·init 실패 재시도·이전 이미지 복원 smoke(#181, 로컬 x64 고정 이미지 쌍), PostgreSQL 16/17 x64 DB suite matrix와 기존 PG18 ARM64 유지(#175; PG16/17 ARM64·운영 백업/복구 아님). 수락: S3 이미지 업그레이드·versioned rollback 실제 실행 witness(#190, 로컬 S3 호환 silo·x64), 수동 opt-in ARM64 이미지 업그레이드 job과 실제 ARM64 실행(#192, 고정 쌍 `d0942f10`→`1e0acbe1` local storage). 기존 C·F 종료. 수락 범위의 한계(새 F 아님): 실제 클라우드 S3 제공자, ARM64 S3·다른 이미지 쌍·최신 제품 이미지. 수락: `/health`·`/ready`·`/metrics`와 compose healthcheck(#202·#208), 0.x release workflow(#206), 단일 명령 사용자 설치(#207: `compose.user.yml`+`.env`, 컨테이너 시작 준비 후 uid 1000 서버, 5개 root 0400 secret 파일, init 서비스 없음; #213 규칙과 일치, pr213 검토 N4 해소). 개발 `compose.yml`(init 서비스)은 개발·소스 빌드 경로로 유지한다. 한계: 같은 컨테이너 root 준비와 uid 1000 서버 경계(컨테이너 root·호스트 Docker 사용자는 secret을 읽을 수 있음), 기본 localhost HTTP, symlink `FVOCI_MEILI_KEY_FILE` 거부(검토 F13, 문서화). 첫 release 게시: `v0.1.0`(`57497e2f`) release 실행 [36433264742](https://github.com/AISFlow/fvoci/actions/runs/36433264742) 성공, 이미지 index `sha256:02380fef1b906eb0be6de6bdbd94f338595ba62ae26b7ef301319d57fad07cfe`(smoke 도구 `06b039e4`). 앞선 실행 36416132900은 테스트 클라이언트 cookie 결함으로 smoke가 실패했고 #218로 수정했다. `v0.1.1`(`71d252a6`, #223) release 실행 [36484451812](https://github.com/AISFlow/fvoci/actions/runs/36484451812)(tag push) 성공, 이미지 index `sha256:419529858229c6792612ee5bda38521ccceac366cec08aee65b4537edb484223`(`:0.1` 동일). `v0.2.0`: tag `<TBD>`, release 실행 `<TBD>`, index `<TBD>`. 수락(0.2.0): 서버 `PR_SET_DUMPABLE 0`(실패 시 기동 거부)으로 uid 1000 helper·`docker compose exec` 세션이 서버 environ·메모리·fd를 읽지 못함(#238; `standalone-install-smoke.sh`의 새 검사는 작성자의 overlay 이미지 실행으로만 확인했고 CI에는 연결되지 않았다. 같은 검사가 `release-smoke.sh`에 있어 release 실행에서 돈다. 운영 진단은 `exec --privileged`). A: 남은 제품 차이 없음 |
| 추가 DB·플랫폼 | PR999 packages/db (SQLite/libSQL/Turso) | 원본 제공 범위와 목표 구분 | G | — | G(미착수 종료, 새 범위 아님): 원본 `39379526`의 앱 연결 경로는 PostgreSQL뿐이다. SQLite/libSQL/Turso는 즐겨찾기·그룹·초기 관리자 어댑터 슬라이스와 SQLite 파일 `VACUUM INTO` 백업/복원 테스트만 있고, 앱 설정 스위치·전체 스키마·서버 기동은 없다(원본 `docs/ops/databases.md`, `packages/db/src/sqlite/access.ts`). 제품 백엔드가 아니므로 Rust main의 PG 전용은 원본 제품 경로와 같고 새 백엔드를 만들지 않는다. 원본 다중 DB를 완료로 간주하지도 않는다 |
| 프론트엔드 | apps/web, packages/editor | 한국어·접근성·기존 흐름 | 수락 | 각 PR E2E | 수락: 첨부 HWP viewer(#187)·PPTX viewer(#188), 격리 Playwright 실패 산출물 보존(#195), HWP 편집 사본 저장 UI(#189), 워크스페이스 활동 설정(#201), 태스크 목록·board load-more 경합(#211·#212). 수락(CI 전용): 실패 group의 redaction된 `server.log`·`browser-summary.txt` 보존(#229), group별 netlink 기록과 Playwright 전 네트워크 settle 대기(#237; 첫 로드 빈 화면의 완화이며 원인 증명이 아니다. 앱은 첫 로드 chunk 실패를 재시도하지 않는다, §5). 측정: 성능 기준선 도구(#214, opt-in). 한계: 실제 OS IME는 Linux X11 witness 범위(§2). A: 남은 차이 없음 |

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
- DB 역할: 소유자 URL은 `fvoci-migrate`와 `--grant-app-role`만 사용한다(사용자 설치에서는 `fvoci-migrate --start`의 준비 단계). 서버는 `DATABASE_APP_URL`만
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
- 프로젝트 문서 그룹 경로(`GET/POST/DELETE /api/v1/workspaces/:workspaceId/projects/:projectId/documents/:id/groups`)는
  등록하지 않는다. 원본 `3937952`는 `wiki()` 쌍으로 등록하지만 코어 `loadWikiDocument`(packages/core/src/group.ts:222-231)가
  projectId 있는 문서를 404로 막아 2xx 경로가 없다(원본 tenant-isolation.test.ts:753-760, 원본 web 호출자 없음).
  Rust는 미등록 API 404로 같은 결과를 내며, 성공할 수 없는 요청의 401/400 대신 404가 되는 차이만 있다
  (`fvoci-evidence/opus-project-document-groups.md`).
- 설치(2026-09-28 사용자 결정, #207·#213): 사용자 설치는 `.env` 하나와 `compose.user.yml`이며 준비는 앱 컨테이너 시작 절차다.
  서버 프로세스는 uid 1000이며 소유자 비밀번호·Meili master key를 갖지 않는다. 개발 `compose.yml`의 init 서비스는 개발 경로다.
- 검색은 사용자 결정으로 원본과 같은 Meilisearch를 쓴다. 서버는 index 범위 scoped key만 받고(마스터 키는
  준비 단계만: 사용자 설치의 root `fvoci-migrate --start`, 개발 stack의 init), Meili 필터는 recall 최적화이며 보안 경계는 PG hydrate다. Meili가 없으면 서버는 기동하고 검색만
  503을 낸다(키 거부 401/403만 기동 실패).
- 협업 helper는 신뢰할 수 없는 Yrs 디코더 격리 때문에 room당 프로세스를 유지한다(I1–I5). 용량은 개수 상한
  대신 메모리 예산 admission과 oom_score_adj로 늘린다(자문 Q4).
- 동시 쓰기 워커는 코디네이터 직접 구현·하위 agent를 포함해 프로젝트 전체 최대 8(2026-09-28 사용자 지시, #217; 기존 5개 슬롯을 먼저 쓰고
  수정 파일·공통 계약·선행 작업이 분리될 때만 6~8개. 이전 3은 #140, 5는 2026-09-27 기록 보존), 무거운 로컬 검사는 한 묶음,
  worktree별 target, 실행별 DB/역할/포트를 유지한다. 읽기 전용 검토·조사는 쓰기 슬롯에서 제외한다.
  독립 검토는 별도 세션이며 기본 최대 2개, 서로 다른 고정 후보가 쌓이면 최대 3개다. 코디네이터 자기 검토를 독립 검토로 표시하지 않는다.

## 5. 알려진 결함·위험

- 협업 caret [1,1] 간헐 실패는 #24(awareness 갱신이 native caret을 덮는 제품 결함)로 수정했다. ACL poll이
  한 틱씩 건너뛰던 결함은 #27로 수정했다(철회 지연 2배 → 설정값).
- 협업 room마다 소유 fence(advisory lock)용 PG 연결(풀 밖)을 하나씩 점유한다. 필요 연결 = room 수 + 앱 풀 + reserve 10이며
  시작 시 `max_connections`를 검사한다(#46). 유지보수 job claim이 실행 중 1개를 추가로 쓰며 reserve 안에 든다.
- collab_product는 디버그 helper·병렬 56 스레드에서 짧은 내부 deadline 테스트가 간헐 실패할 수 있다(CI 정상).
  테스트 harness 수정과 용량 작업에서 함께 다룬다.
- 실제 OS IME는 Linux X11 IBus hangul 2벌식·Chromium의 실제 XTEST witness로 수락했다(§2). Windows·macOS·모바일·특정 기기 입력은
  그 witness 범위 밖의 한계로 기록한다(새 필수 조건 아님). 합성 이벤트 성공을 IME 검증으로 표시하지 않는다.
- 제한기는 프로세스 로컬·직접 socket IP 기준이며 신뢰 프록시·분산 제한은 없다. reverse proxy 뒤에서는 모든 사용자가 proxy 주소
  하나의 bucket을 공유하므로 로그인 IP당 30/5분, 초대 수락 IP당 60/5분(#236)에 대량 온보딩이 걸릴 수 있다. 한 익명 client의 새 키
  폭주가 다른 한도(로그인·MFA 재인증·setup·share)를 초기화하던 결함은 #233(키별 window·최대 namespace 축출)으로 수정·수락했다.
  축출은 키가 가장 많은 namespace의 hit가 가장 적은 키부터이므로 폭주 namespace가 최대가 되기 전까지는 더 큰 다른 namespace의 키를
  축출할 수 있다(`src/http/rate_limit.rs`). IPv6 주소 회전(주소별 key)과 계정별 로그인 제한은 정책 항목으로 미포함이다.
- 협업 receipt/이벤트/감사 누적은 원본과 같이 보존 정책이 없다. 첨부 중단 업로드 정리는 #63·#65로 수락했다.
- 전역 보안 헤더(원본 nosecone CSP·Referrer-Policy 등)는 #81로 수락했다(공유 응답의 개별 CSP·no-referrer 유지).
- 2026-09-26 감사의 identity link issuer·link tx 세션 재검증·ENCRYPTION_KEYS 복구 검증·`--verify-storage` preview·
  document-extract child env 상속은 #90으로 수정·수락했다.
- 가져오기 POST는 요청당 최대 수백 MiB를 버퍼링하며 프로세스 전체 동시 수 상한이 없다(원본도 버퍼링). Node convert
  helper는 #101·#103·#106·#107로 Rust child로 대체돼 제품 이미지에서 제거됐다.
- collab_product `collab_empty_byte_update_is_rejected`가 #66 CI에서 1회 auth 전 close로 실패했다(로컬 7회
  재현 실패, 재실행 성공). #67로 close code를 남기며 재발 시 원인을 닫는다.
- 협업 helper의 OOM backstop: #238부터 helper가 `main` 첫 단계에서 자기 `oom_score_adj=1000`을 쓰고 서버가 첫 응답 뒤 읽어 확인한다
  (비-dumpable 부모의 `pre_exec` 쓰기가 실패하던 경로 제거). 값이 1000이 아니면 서버가 1회 경고하며, 이때 cgroup OOM이 서버 대신 helper를
  고른다는 보장은 없다(컨테이너 mem_limit·helper별 AS/RSS 한도가 상한). 이전 관측은 AppArmor docker-default 환경(예: GitHub runner)이
  쓰기를 거부한 것이며, #238 경로의 AppArmor 재측정은 실행하지 않았다. 새 서버와 이전 helper binary를 섞으면 backstop이 없고 경고한다
  (같은 이미지면 해당 없음). #46, #238
- 검색 색인 소비자는 outbox chunk(기본 최대 100 event)를 묶어 Meili 작업을 enqueue하고 chunk 끝에서 한 번 모든 task를
  기다린다(`src/search/index.rs` `deliver_batch`·`MeiliBatchSink`, `wait_meili_tasks`). 이전의 "단건마다 대기, 약 0.5 event/s"
  기술은 배치 전 관측이라 더 이상 맞지 않는다(`grok-fresh-install-port-audit.md`). 남은 위험: 소비자는 chunk의 Meili task가
  성공할 때까지 cursor를 진행하지 않으므로(fire-and-forget 아님) Meili가 느리면 색인이 지연된다. 배치 후 부하 처리량은 측정하지 않았다.
- 성능 기준선(`opus-perf-baseline.md`, 소스 빌드 `1101e21b`, 단일 호스트): 협업 room 상한(이미지 기본 30, 사용자 compose 64)이
  차면 새 본문 join이 1013으로 닫히고 클라이언트가 backoff 없이 재연결하며, room은 마지막 클라이언트 후 30 s에 해제되므로 새 본문이
  최대 ~30 s 비어 보인다(서버 로그 없음). 태스크 메타의 다른 브라우저 반영은 SSE 750 ms poll이 지배한다(p95 ~0.75 s).
  첨부 viewer는 첫 표시 전 ~300 ms 유휴 구간이 있다. 첨부 viewer 첫 표시는 #221, room 상한 동작은 #225로 개선했다(최종 head·게시 이미지 재측정 없음). SSE poll은 설계 결정 대기다.
- 초대·공유·ICS 경로 토큰과 OIDC code/state가 요청 trace span의 원시 URI로 debug 로그에 남던 문제는 #178로 수정·수락했다
  (span에는 method·`MatchedPath` 템플릿 또는 고정 fallback·version만 기록). 임의 애플리케이션 로그 전체 감사는 아니다.
- 첫 로드 빈 화면(#221 이후 CI 간헐): #229 진단이 Chromium의 boot chunk 요청 `net::ERR_NETWORK_CHANGED` 중단을 포착했고(run 36472777008,
  서버는 정상 응답), #233 merge의 main push(run 36488304394, project-trash-flow)도 같은 서명이었다. #237은 Playwright 전 네트워크 settle
  대기와 group별 `net-events.log`를 추가한 완화이며 원인 증명이 아니다. 다음 실패의 `net-events.log`에 중단 요청과 ms 단위로 맞는
  주소/링크 이벤트가 없으면 가설은 반박된다. 앱은 첫 로드의 chunk 실패를 재시도하지 않으므로(`apps/web/src`에 preload 오류·chunk 재시도
  처리 없음) 사용자 측 네트워크 변경 때도 빈 화면이 될 수 있다(새로고침; 0.2.0 노트의 Known limitations에 기록).
- sqlx 0.8.6 `Transaction::begin`은 `BEGIN`을 보낸 뒤 depth를 올리므로 `BEGIN` 도중 취소된 요청이 서버 측 트랜잭션을 연 채 연결을
  pool에 돌려준다(upstream 수정 #3980은 0.9에만). #231이 앱 pool `before_acquire`에서 재사용 idle 연결마다
  `SELECT now() OPERATOR(pg_catalog.=) statement_timestamp()`(simple protocol, `test_before_acquire(false)`로 ping 대체)를 보내 열린·중단
  트랜잭션 연결을 닫는다(`src/db/pool.rs`). 비용은 작성자 로컬 측정(release build, PG18 Docker, 잡음 있는 호스트) `SELECT 1` 기준 순차
  +10–12%, 포화 +1–24%이며 게시 이미지 측정은 없다. 후속: sqlx 0.9 업그레이드 후 검사 제거, `try_acquire`/`try_begin` 우회를 clippy
  `disallowed-methods`로 금지.
- #231의 acquire 검사 문은 PG 문 기록에도 남으므로 문 목록 전체를 비교하는 테스트가 pool 상태에 따라 달라진다: `086f2b4d` main push의
  `postgres-pg16-b`에서 `collections_integration` `wiki_collection_can_edit_uses_one_set_based_permission_lookup`이 3행/21행 문 목록의
  검사 문 1개 차이로 실패했다(run 36498519146; 같은 테스트는 `ea0e85a8`·#232 PR 실행에서 통과). 테스트 전제 결함이며 제품 영향은 없다.
  #239(`e600047c`)가 비교에서 검사 문을 제외해 수정했다(query 자체의 문은 그대로 정확히 비교).
- 읽기 경로는 #231 이후 xid를 잡지 않지만 `purge_workspace` 같은 긴 쓰기 트랜잭션은 여전히 cluster xmin을 붙잡아 모든 tenant의 SSE·outbox
  진행을 늦출 수 있다(측정 대상). 구·신 서버 혼합 시 검색 색인·WIP lock namespace가 서로 직렬화되지 않는다(제품은 rolling upgrade 거부).
- SSE settled-horizon cursor(#234)의 64 stream poll 비용은 전후 측정하지 않았다.
- 메일 수신자 단위 전달(#232)의 수락 수신자 목록은 프로세스 메모리(최근 64 mail event, `ACCEPTED_EVENTS_KEPT`)에만 있다. 한 event의 재시도
  사이에 재시작하거나 다른 replica가 lease를 넘겨받으면 이미 수락된 수신자에게 다시 보낼 수 있다. 영속화는 migration이 필요한 후속이다.
- digest(#232)는 15분 예산(`DIGEST_TIME_BUDGET`), 응답 없음·4xx 실패 5회(`DIGEST_DOWN_STREAK`), 수신자를 특정하지 않는 5xx 20회
  (`DIGEST_REFUSAL_STREAK`; 발송 성공이나 한 mailbox 거절이 두 수를 초기화)에서 walk를 멈추고, 못 보낸 행은 다음 날 sweep으로 넘긴다.
  알려진 한계(`src/mail/digest.rs`): 매일 같은 수신자들이 지속 4xx(예: `452 4.2.2` 용량 초과)나 향상 상태 코드 없는 relay의 bare 550으로
  실패하면 walk가 매일 같은 행에서 끝나 그 뒤 행은 digest를 받지 못한다.
- `restore.sh`의 `\gexec` `CREATE ROLE … PASSWORD`는 PostgreSQL `log_statement`가 `ddl` 이상(기본 off)이면 서버 로그에 남는다(#233, 스크립트에 문서화).
- identity(#236) 잔여: team workspace 관리자는 자신이 제어하는 IdP로 cross-site `GET /api/v1/auth/oidc/generic/start?workspaceId=` 또는
  `GET /api/v1/auth/sso?slug=`를 통해 login CSRF를 일으킬 수 있다(로그인 start는 GET 유지; team workspace는 instance admin만 생성).
  사용자 결정 대기: ACC-1(OIDC로 만드는 초대 계정에 검증된 초대 이메일 요구; 구현 후 `23967a06`으로 되돌림 — Naver는 `email_verified`를
  보내지 않고 Kakao는 자주 생략. 원본 `39379526` `acceptInviteWithIdentity`도 provider email 일치·검증을 요구하지 않는다), Share part B
  (생성자 권한을 잃은 뒤에도 링크 유지, 원본과 동일). Share F1 A는 생성 시점 검사이며 기존 링크는 재검사하지 않는다. 개인 workspace에
  저장된 SSO 행은 비활성이지만 설정 화면이 그 영역을 숨기므로 `DELETE /api/v1/workspaces/{id}/oidc` API로만 지울 수 있다. 후속(검토 nit):
  link에서도 `Origin` 없는 요청 거부(초대 start와 같이), 테스트의 fetch mode 고정, 초대·계정 페이지가 script 시작을 유지하는지 source 검사.
  OIDC는 로컬 test provider만 사용했고(실제 외부 IdP 미검증), 브라우저 시작은 Chromium 153의 미커밋 1회 실행으로만 확인했다.
- 서버 비-dumpable(#238): uid 1000 helper·`docker compose exec`(uid 1000, 또는 `CAP_SYS_PTRACE` 없는 기본 root)는 서버 environ·메모리·fd를
  읽지 못하며 core dump도 남지 않는다(`fs.suid_dumpable`과 무관). 운영 진단은 `docker compose exec --privileged fvoci …`(CAP_SYS_PTRACE)가
  필요하고 이미지에는 gdb·strace·lsof가 없다. `perf -p`는 `CAP_PERFMON`/`CAP_SYS_ADMIN`으로 만든 컨테이너가 필요하다. helper는 여전히
  uid 1000 파일 접근(첨부 저장소 `/data/storage`, scoped 검색 키)을 서버와 공유하고 helper끼리는 dumpable이다(호스트가 같은 uid ptrace를
  허용하면 서로 attach 가능). 후속(검토 nit): `RUNNING.md`의 perf/seccomp 문구 정밀화.
- TS 데이터 이전은 사용자 확인으로 범위 밖이다(§2).

## 6. 재개

### 6.1 인계 체크포인트 (2026-09-29 `<TBD>` KST, 0.2.0 게시 후, Claude Code Opus 5.5 코디네이터)

다음 세션은 이 절과 실제 `origin/main`·열린 PR·브랜치·release를 대조한 뒤 인수한다. 로컬 evidence(`/home/kinesis/orca/fvoci-evidence/*.md`)는
보조자료이며 재개에 필수는 아니다. 이전 체크포인트(2026-09-29 00:10 KST, Fable 코디네이터 세션 종료)는 이 파일의 git 이력에 있다.

- **확인한 origin/main**: `<TBD>` (= 0.2.0 준비 PR #`<TBD>` merge). main push CI: `<TBD>`.
  직전 관측: `086f2b4d`(#232 merge)의 `rust-ci-gate`는 `postgres-pg16-b` 문 목록 비교 테스트의 상태 의존 실패로 빨간 상태였고 #239
  (`e600047c`)로 테스트를 수정했다(§5). `0a04131c`(#233 merge)의 `web-ci-gate`는 #237 이전의 `net::ERR_NETWORK_CHANGED` 첫 로드 빈 화면으로
  빨간 상태였다. `1b8666a5`(#238)·`31459b3c`(#236)의 main CI는 39 성공·1 skip, 5개 gate 성공. `e600047c`(#239) main CI: 39 성공·1 skip, 5개 gate 성공.
- **게시된 버전**(태그·이미지·Release는 이동/덮어쓰기하지 않는다):
  - `v0.1.0` = `57497e2fce9efd9e953592f539003c1e0c52d7e2`, release 36433264742, index `sha256:02380fef1b906eb0be6de6bdbd94f338595ba62ae26b7ef301319d57fad07cfe`.
  - `v0.1.1` = `71d252a6e593adce2b960cefd60b8abe6171df41`, release [36484451812](https://github.com/AISFlow/fvoci/actions/runs/36484451812)
    (tag push), 이미지 `ghcr.io/aisflow/fvoci:0.1.1@sha256:419529858229c6792612ee5bda38521ccceac366cec08aee65b4537edb484223`
    (amd64 `sha256:4a277883cb70f0e4522974b2317c83cca1387f8405a6116876ed6f53efd5c3b5`, arm64 `sha256:462c9a1ce8aee08d8d6291e0080bc36d32bc4c12fc36460f4dddf5121313086f`), `:0.1` 동일.
  - `v0.2.0` = `<TBD>`, release `<TBD>`, 이미지 `ghcr.io/aisflow/fvoci:0.2.0@<TBD>`
    (amd64 `<TBD>`, arm64 `<TBD>`), `:0.2` `<TBD>`. pre-release https://github.com/AISFlow/fvoci/releases/tag/v0.2.0.
- **0.2.0 포함 변경**(v0.1.1 이후): 제품 #231 #232 #233 #234 #235 #236 #238, CI·테스트 #230 #237 #239. migration 044(#236),
  workspace SSO redirect URI `/api/v1/auth/sso/{workspace_id}/callback`. 새 `.env` 값 없음, 설치 파일은 버전·source commit·image digest만 바뀐다.
- **열린 PR**: `<TBD>`.
- **다음 후속(비차단, 순서는 코디네이터 결정)**: sqlx 0.9 업그레이드와 acquire 검사 제거·`disallowed-methods`; #236 검토 nit(link의 `Origin`
  없는 요청 거부, fetch mode 테스트 고정, script 시작 source 검사)와 개인 workspace SSO 행 정리 경로; #238 `RUNNING.md` perf/seccomp 문구 nit;
  메일 수락 수신자 영속화(migration); digest streak 한계 재검토; SSE 64 stream poll 비용 측정; `purge_workspace` xmin 영향 측정;
  AppArmor docker-default에서 helper OOM backstop 재측정; 첫 로드 빈 화면 재발 시 `net-events.log` 대조; Prometheus 2단계(room 점유·거부·회수 지표);
  0.1.1 게시 파일에서 0.2.0으로의 이미지 업그레이드 검증(`<TBD>`).
- **사용자 결정 대기**: ACC-1, Share part B, team workspace 관리자의 GET start login CSRF 수용 여부, #149 S3 전송 정책(변경 없음, 수락된
  API 프록시 유지), SSE 750 ms 폴링 설계.
- **외부 검증(사용자 환경)**: 실제 IdP(workspace SSO 새 callback 포함)·인증 앱 스캔·다른 푸시 제공자, Chromium 외 브라우저의 OIDC 시작,
  Mac Docker Desktop·rootless·Podman.
- **이전 체크포인트에서 유지**: 설치 방향(확정) — 사용자가 짧은 `.env`를 작성하고 `docker compose up -d`, 기본 서비스 fvoci·postgres·meilisearch,
  앱 시작 절차가 준비(검증·migrate·grant·검색 키) 담당, 정상 서버는 uid 1000·제한 앱 역할; 무설정 bootstrap 서비스·미배포 초안 호환 계층을
  다시 만들지 않는다. 이 문서의 성능 수치는 소스 빌드의 로컬 측정이며 게시 이미지(v0.1.x·v0.2.0)에 대한 성능 측정은 없다. 보류: `flushDelay`
  50/100 실험, DocumentView 구독 분리, Svelte·Astro·Valkey·대규모 room 재설계.
- **로컬 자료·자원**: `<TBD>`.
- **자동 연결**: `<TBD>`. 진행 중 릴리스 `<TBD>`.

### 6.2 재개 절차

1. `AGENTS.md` → `.agents/environment.md` → 이 문서 → 열린 PR·`origin/main` CI → `git worktree list`·각 worktree `git status` 순으로 실제 상태를 확인한다.
   Orca가 있으면 `worker-list --run run_b01d432a9dee`도 대조하되, 재개를 그것에만 의존하지 않는다.
2. 진행 중 worktree의 미수락 커밋을 보존하고 같은 작업을 중복 배정하지 않는다.
3. 로컬 DB 검사: `scripts/start-test-postgres.sh cargo test --locked --offline --no-fail-fast
   --features db-tests --test <suite>`. 서버 실행 전 `fvoci-migrate` → `fvoci-migrate --grant-app-role <role>`.
4. 설치 확인: 개발 stack `scripts/install-smoke.sh`, 사용자 설치 `scripts/standalone-install-smoke.sh`(RUNNING.md "Install"); 릴리스 절차 `docs/RELEASING.md`.
5. 다음 기능은 위 대응표의 잔여 행에서 의존성이 준비된 사용자 흐름을 먼저 고른다.
