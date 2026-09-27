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
이 기록의 수락 기준 main은 `6ba876503e2a8340c0c21f8f110d2ec14f67f8b3` (#141, 2026-09-27, main 5개 workflow 성공)이다. 열린 제품·CI PR은 아래에
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

검증 기준: 각 PR의 필요한 실제 검사·원격 CI와 별도 세션의 독립 검토를 고정 SHA에서 확인한다.
과거 Opus/Fable 검토는 당시 범위의 근거로 보존하며, 현재 역할은 AGENTS.md를 따른다.
최신 실행·소유권·검증 SHA·인계 포인터는 `/home/kinesis/orca/fvoci-evidence/coordinator-handoff-2026-09-26.md`에 둔다.

현재 작업·잔여 범위:
- 기준 main 시점에 수락 대기였던 #142·#146·#148은 이후 위 표의 고정 HEAD로 수락·머지됐다. 당시 관측은 git 이력에 보존한다.
- 수락 대기(C, 2026-09-27 14:55 UTC 관측, 미수락): calendar drag PR [#162](https://github.com/AISFlow/fvoci/pull/162)
  HEAD `548739735922df772b0077128b2a08f972949d7a`, 첨부 다운로드 PAT 허용 PR [#164](https://github.com/AISFlow/fvoci/pull/164)
  HEAD `a166f9525d13c64919807751614eebfb8ff38edf`, 서버 메시지 `i18n.overrides` 소비 PR [#165](https://github.com/AISFlow/fvoci/pull/165)
  HEAD `7f3e24779f967092cfdb1b901edffa10dd48b618`(CI 진행; 컨테이너 빌드의 Docker Hub 연결 재설정 환경 실패 관측, 제품 검사 미실행).
- 분류 근거·행별 종료 조건: `/home/kinesis/orca/fvoci-evidence/opus-decision-status-cleanup.md`.
- 문서 변환의 정상 Rust 경로는 수락됐으며 Node 구현 복원은 필요 없다. 개발·CI의 Node/Python과 독립 reader는
  제품 런타임과 별개다. 필수 백업·복원 자체 검증은 Rust이며 pg_dump/pg_restore·얇은 실행 스크립트는 유지한다.
- migration 037·038·039·040은 main에 수락됐다. 번호를 다시 배정하지 않는다.
- 잔여 기능·검증: 실제 브라우저 PushManager 및 외부 푸시 서비스(FCM 등) 발송(F),
  S3 브라우저 전송 방식(E, 사용자 결정 대기 [#149](https://github.com/AISFlow/fvoci/issues/149)), settings 소비자 `i18n.overrides`(B, #165 수락 대기; 문서 메뉴 `features.ai` 소비는 #159 수락),
  #77 외부 제공자 검증의 미실행 범위(F), 컬렉션 `dueBefore` 시간대(D, 실제 DB 검증은 다른 워커의 진행 task)·calendar drag(C, #162)·wiki 컬렉션 권한 N+1(#76 S3·S4, 현재 코드 재확인 전),
  preview-html의 미추출 파일 즉석 격리 parse·HWP viewer(rhwp WASM)(B), 추가 DB(G 추적).
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
| 설치·로그인·세션·프로필 | identity/routes.ts, core/auth.ts | 활성 사용자, 철회, 본문+이벤트+감사 원자성 | 부분 | #1, #34, #53, #69, #72 | 수락: 비밀번호 재설정(#53), 탈퇴·익명화·비밀번호/이메일 변경·magic link·export(#69), 설정 기반 비밀번호 최소 길이(#72), TOTP MFA·OIDC·workspace SSO(#77), MFA 등록 QR(SVG)·수동 키 UI(#84). F: 실제 인증 앱 스캔, #77 실제 외부 IdP 미실행 범위(브라우저 구조 검사와 구분). 종료: F 실행 증거 |
| 워크스페이스 | domains/workspaces | 현재 역할·철회 경합·RLS·풀 컨텍스트 | 수락 | #4, #39, #56, #61, #63, #110, #134 | 수락: counts·owner 전용 삭제(#56), 30일 purge 실행기(#61), S3 저장소 purge(#63), 원본 사용권별 workspace/guest/storage quota(#110), workspace ZIP 내보내기(#134). A: 남은 차이 없음 |
| 멤버·초대 | invitation.ts, quota.ts, consent.ts | 좌석 한도(모든 billable 경로)·토큰 단일 사용·역할 상한 | 수락 | #21, #53, #69, #72, #77, #84 | 수락: 초대 메일(#53), 초대 수락의 legal consent 428·`defaults.user` 적용(#72), 탈퇴·관리자 삭제 시 보낸 pending 초대 정리(#69·#84), 초대 수락 시 MFA challenge·OIDC 초대 수락(#77). A: 알림/사용자 기본값·계정 삭제 정리·E2E SQL fixture(없음). G: pending 목록/철회 API(원본 contracts·UI에 없음) |
| 그룹·권한 통합 | policies.ts effectivePermission, project/document_members(user XOR group) | 리소스별 단일 권한 함수 | 수락 | #23, #39, #50 | 후속: collab 프레임당 권한 재조회 축소·collab_delivery의 그룹 join 사본·설정 UI `canManage` DTO |
| 프로젝트 | domains/projects | 비공개 접근(workspace admin 제외)·lead/멤버 제거 경합·원자성 | 수락 | #13, #39, #52, #76, #78, #80, #83, #161 | 수락: 프로젝트 문서 협업·trash/restore/sort·archive·30일 purge(#78), 프로젝트 문서 body/children/ancestors/backlinks/duplicate(#83), collection/view 복제(#52·#76), 프로젝트 문서 첨부(#80)·태그 route. 기존 B 표기 정정(2026-09-27): 프로젝트 문서 리비전 API·UI·forward 복원은 #161 수락, 프로젝트 그룹 route(`src/http/routes/groups.rs`)·UI(`project-groups.tsx`, `workspace-groups-flow` E2E)는 #39에서 이미 수락(A). A: 남은 차이 없음 |
| 태스크 | domains/tasks, core/task.ts, workflow.ts | 권한·버전·WIP·반복 회차 원자성·키셋 커서 | 부분 | #13, #19, #26, #38, #40, #47, #60, #142, #163 | 수락: activity feed(#60), 본문 협업·block patch·수동 리비전(#104), origin UI/API(#117), archive 전 본문 영속화(#121). Gantt 조회·편집 UI(#130)는 수락. 수락(기준 이후): board 그룹 paging·drag-and-drop(#142), 태스크 메타 변경 SSE 전달(#163). D: `dueBefore` 시간대(공유·컬렉션 행에서 추적). 종료: 해당 D 수정 |
| 일정·ICS·휴일 | routes.ts ics/holidays | 일정 의미 | 수락 | #49, #76 | 수락: 휴일·ICS 피드(담당 태스크, #49), 저장 calendar view 기반 ICS 분기(#76). A: 남은 차이 없음 |
| 위키 문서 | domains/documents, core/document.ts | 현재 문서 권한·트리 잠금 순서 | 수락 | #5, #23, #39, #66, #70, #78, #83 | 수락: 가져오기·내보내기(#66), 공유 링크(#70), trash 30일 purge(#78), body/block patch/children/backlinks/duplicate/flat(#83). 수락: office·Notion 가져오기(#85), Rust 변환·내보내기/doctor·Node 없는 제품 이미지(#101·#103·#106·#107). 수락: 가져오기 복구 보상(#112), 문서·태스크 템플릿(#118). A: 남은 차이 없음 |
| 리비전 | documents/revisions.ts, core/revision.ts, collab applyRestore | 복원은 room actor의 forward system update, durable 후 broadcast | 수락 | #25, #104, #123, #128, #151, #161 | 수동·session·scheduled 리비전과 자동 보존 개수 제한은 수락. A: 리비전 route의 `document_permission` 인가. 기존 D 의심(writer-stale 후 연결 없는 room의 수동 캡처가 오래된 본문 저장): #151에서 실제 앱 역할 PG·WebSocket 재현 후 수정·수락. 프로젝트 문서 리비전(#161) 수락. A: 남은 차이 없음 |
| 댓글 | comments/routes.ts, core/comment.ts | 문서 XOR 태스크·부모 활성·권한 | 수락 | #28, #47, #58, #146 | 수락: 그룹 멘션·프로젝트 문서 댓글(#58; 그룹 멘션은 원본 snapshot과 달리 전달 시점 재전개). 수락(기준 이후): 이중 DELETE 중복 이벤트·resolve 경합·동시 trash된 태스크 댓글(#146). A: 남은 차이 없음 |
| 협업 | domains/collab, React/Tiptap | provider envelope·철회·CRDT 정본·persist barrier·writer generation·재시작 복원 | 부분(opt-in) | #6, #7, #18, #24, #27, #39, #46, #131 | 수락: room 용량(기본 30, 64 검증; §2), helper SIGKILL 후 복구(#131). F: 실제 OS IME. B: opt-in 해제는 정책이 아니라 수락 조건 — 제품 이미지는 helper를 설정하나 RUNNING.md는 actor panic/rejoin 수락 전 opt-in으로 기술. 종료: F 증거, 남은 panic/rejoin 수락 기준 검증 후 문구 정정 |
| 첨부 | domains/attachments, packages/storage | 부모 권한·원본 bytes·원자 완료·취소 | 부분 | #10, #39, #58, #63, #65, #80, #148 | 수락: viewer route(#58), S3 프록시·중단 업로드 GC(#63, #65), 태스크·프로젝트 문서 부모·DELETE·quota·이미지 preview·저장 추출문 preview-html(#80). 수락: `--verify-storage` preview 객체·크기 확인(#90). E: S3 브라우저 전송 방식(프록시 유지 vs presigned, [#149](https://github.com/AISFlow/fvoci/issues/149); 결정 전 현재 프록시 유지). B: 미추출 파일의 preview-html 즉석 격리 parse, HWP viewer. 수락(기준 이후): 업로드 완료 재시도·요청 상한 413/524(#148). D: 다운로드가 세션 전용(원본 계약은 PAT 허용; C 수정 PR #164 수락 대기). 종료: E 결정 반영, B 구현, D 수정 수락 |
| HWP/HWPX 추출 | 원본 추출 경로, rhwp e8800c8 | 부분/손상/미지원을 빈 본문 성공으로 바꾸지 않음·자원 한도 | 부분 | #2, #8, #9, #11, #35 | G: HWP 썸네일(원본 thumbnail은 이미지 MIME만). B: lease 만료·재시도 결과 게시 경계. 종료: B 회귀 |
| 검색·색인·AI | domains/search, packages/search | 검색에서도 인가·철회·색인 복구 | 부분 | #29, #30, #35, #48, #57, #58, #82, #83, #159 | 수락: 워크스페이스·전역 검색, 댓글 hit, outbox 색인 배치(#57), 복구 후 rebuild, PAT scope 검색(#83), 의미(벡터) 검색(#82), 첨부 hit의 viewer 이동(#58, search E2E). A: 검색·색인·의미 검색은 수락. 수락(기준 이후): 문서 메뉴의 `features.ai` 소비(#159). B: AI 결과를 문서 본문에 적용하거나 태스크를 생성하는 UI(현재 결과 미리보기만 제공). 종료: B 연결·관련 인가/편집·브라우저 회귀 |
| 알림·outbox·메일·webhook·연동 | domains/notifications, packages/jobs | 커밋 후 전달·중복/재시도 | 부분 | #31, #35, #45 | 수락: 앱 내 알림, 메일·digest(#53, #61), webhook·GitHub App·AI 동작(#74). 수락: Web Push와 로그아웃 시 브라우저 연결 해제(#139). F: 실제 외부 푸시 서비스·브라우저 PushManager. G: requeue 운영 API/UI(원본도 없음). 종료: F 증거 |
| 공유·즐겨찾기·최근·태그·컬렉션 | 해당 routes | 공유 링크 권한 | 부분 | #70, #72, #76, #84, #138, #142 | 수락: 즐겨찾기·최근·공유 링크·공개 공유 페이지·PDF, 공유 정책(#72), 태그·컬렉션·저장 view(#76), 대화상자 정책·`/s/:token` head meta(#84), 공유 첨부 preview(#80). 수락: `HEAD /s/:token` 보안 헤더·빈 본문(#138). 수락(기준 이후): board 그룹 paging·drag-and-drop(#142). C: calendar view drag(원본 web collection-drag, PR #162 수락 대기). D: 컬렉션 `dueBefore` 시간대(#76 검토 S3; 정적 확인상 컬렉션 query는 actor 시간대, 태스크 목록·layout은 UTC. 실제 DB 검증·수정은 다른 워커의 진행 task, 사용자 결정 불필요). 종료: #162 수락, D 실제 검증·수정 |
| 동의·감사·사용권·관리 | legal, auth.consents, admin.audit, packages/ee | 동의 gate·증거·권한 | 부분 | #72, #84, #159 | 수락: 관리 API·instance settings·법률 문서·동의·428 gate·branding(#72), 관리자 사용자 삭제 예약/취소(#84). 수락: 사용자가 원본 정책 보존을 확정한 사용권·quota(#110), 운영자 정보(#119), security.txt(#120), 브라우저 오픈소스 고지(#122). 수락: settings `embed`(#124)·`attachmentPreview`(#80) 소비, 문서 메뉴 `features.ai` 소비(#159). B: `i18n.overrides` 소비자(C: PR #165 수락 대기). G: 사용권 issuer trust(원본도 비어 있음). 종료: B 연결 |
| 제품 MCP·CLI·백업·복구 | init.ts, backup.ts, doctor.ts, MCP | 프로토콜·복원 | 수락 | #32, #31, #35, #63, #81, #84, #90, #109 | 수락: 컨테이너 설치 백업·복구(outbox cursor 재기준·검색 rebuild 포함), S3는 스크립트 백업 거부·복구 후 `--verify-storage`(#63, branding #84). 수락: 제품 MCP·CLI·doctor(#81), 키 fingerprint·`--verify-secrets`(#90), Rust 백업 manifest/preflight·키 검증 공유(#109). 개발 Python oracle는 유지. A: 남은 차이 없음 |
| 설치·배포 산출물 | infra/app, compose | 비특권 실행·migrate/grant 분리·helper 포함 | 부분 | #17, #20, #29, #32, #35 | B: 운영 TLS/secure cookie 안내, Rust 버전 간 업그레이드 경로, 원본 지원 PostgreSQL 16/17 검증(CI·compose는 18.3만). 종료: B 문서·검증 |
| 추가 DB·플랫폼 | PR999 packages/db (SQLite/libSQL/Turso) | 원본 제공 범위와 목표 구분 | 미착수 | — | PG 우선; 원본 다중 DB를 완료로 간주하지 않음. G(추적 유지): 원본도 SQLite·libSQL·Turso를 목표로만 두고 전체 앱 백엔드로 선택할 수 없음. 제외하지 않으며 원본 계약 범위가 정해지면 B |
| 프론트엔드 | apps/web, packages/editor | 한국어·접근성·기존 흐름 | 부분 | 각 PR E2E | B: 위 B 항목의 UI. 종료: 각 B 항목 UI |

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

## 4. 유지하는 결정과 의도적 차이

- 작은 단일 Rust 서버(axum·SQLx) + PostgreSQL, 협업은 Yrs native engine, HWP는 rhwp native helper.
  프레임워크·CRDT·HWP 구현체를 다시 비교하지 않는다.
- DB 역할: 소유자 URL은 `fvoci-migrate`와 `--grant-app-role`만 사용한다. 서버는 `DATABASE_APP_URL`만
  받고 기동 시 스키마 버전(`schema_migrations`, 앱 역할 SELECT 전용)이 컴파일 버전과 같아야 한다(#20).
  권한 적용은 단일 트랜잭션이며 수락된 migration 파일은 SHA-256으로 고정한다(#16).
- 협업은 검증된 opt-in 범위만 수락했다. 신뢰하지 않는 문서 parser는 요청 처리 프로세스와 분리된 helper다.
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
- 실제 OS IME·특정 기기 입력은 미검증이다. 합성 이벤트 성공을 IME 검증으로 표시하지 않는다.
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
