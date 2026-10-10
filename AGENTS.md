# FVOCI

Rust 서버(Tokio, axum 0.8, SQLx, Serde, tracing), UI는 Vue 3 + Nuxt UI + Vite, TypeScript는 Bun(`.bun-version`). DB는 PostgreSQL·로컬 SQLite·원격 libSQL/Turso. 협업 문서는 Yrs. 범위·수락 `docs/rewrite.md`, 설치·실행 `RUNNING.md`, 발행 `docs/RELEASING.md`(사람만 명시 시작). 수락 정본은 PR #347 체크포인트 `<!-- fvoci-current-checkpoint -->`와 #331. 레포 문서에 현재 head SHA·run ID·세션·로컬 경로를 적지 않는다.

## 어디를 보나

- 스키마: `migrations/postgres/060`, `migrations/sqlite/060`, `src/db/migrate.rs`
- CI 선택: `scripts/ci_selection.py`. 게이트: `.github/workflows/{rust,web,install,documents,collab-engine}.yml`
- 웹 정적 검사: `.github/workflows/web.yml` `web-static`, 루트 `package.json`. Turso: `docs/testing-turso.md`
- 프론트 기능·상태는 `fvoci-vue-implementation`, 시각·CJK는 `frontend-design`, Rust 구현은 `fvoci-rust-slice`다. 인증·인가·DB·migration은 `fvoci-db-security`, PostgreSQL 성능은 `fvoci-postgres-performance`다.
- 표준·SDK 선택은 `fvoci-standard-implementations`, 원본 호환 조사에는 `fvoci-source-contract`, 설치·native child·build 경계에는 `fvoci-runtime-boundaries`를 쓴다. 검사 선택은 `fvoci-fast-verify`, 제출·재개·병합은 `fvoci-handoff`다.
- Python·shell 하네스의 Rust(xtask)·TypeScript(Bun) 이전은 `fvoci-runtime-boundaries`와 `fvoci-fast-verify`를 쓴다. parser·SDK를 교체할 때만 `fvoci-standard-implementations`의 해당 절을 추가한다.
- 스킬 정본은 `.agents/skills/<이름>/SKILL.md`다. 이름·description으로 해당 작업을 고르고 필요한 본문과 참조만 읽는다. 혼합 변경은 바뀌는 경계만 조합하며 전체 스킬을 일괄 읽지 않는다.
- `fvoci-role-*`는 명시 호출만 한다. Codex에는 각 스킬의 `agents/openai.yaml` invocation 정책을 두고 Claude Code용 frontmatter와 구분한다. 다른 실행 환경에서는 지원하는 선택·호출 방식을 확인한다. 호출 정책은 실행 권한을 부여하지 않는다.

## 완료 조건

경로가 바뀌면 그 명령이 exit 0이다.

- `src/`, `crates/`, `migrations/`: `cargo fmt --check`와 그 타깃의 `cargo test --locked --offline`
- `apps/web/`, `packages/` TS·Vue: 루트에서 `bun run lint`, `bun run format:check`, 해당 스크립트가 있는 패키지에서 `bun --bun run typecheck`, `bun run test`를 실행한다. 자체 스크립트가 없는 공유 패키지는 바뀐 계약을 소비하는 패키지의 검사로 검증하고 cwd와 선택 근거를 남긴다. bare `bun test`는 쓰지 않는다.
- `.github/workflows/`, `scripts/ci_selection.py`: `bash scripts/test-ci-selection.sh`
- 인가·RLS·잠금·원자성: 실제 DB·앱 역할. `TEST_DATABASE_URL`이 없으면 실패로 남긴다.

되돌릴 수 있고 영향이 작은 변경에는 구현을 그대로 반영하는 테스트를 새로 쓰지 말고, 변경에 맞는 테스트와 필수 검사가 통과하면 새 문제가 없는 한 테스트를 넓히지 마라.

그 SHA의 결과는 PASS, FAIL, NOTRUN, MISSING이다. CANCELLED·SKIP은 PASS가 아니다. 이전 SHA를 현재 head에 합치지 않는다. 리뷰 판정은 원격 CI를 대신하지 않는다. 이미지·경로·ref·레지스트리 검사는 커밋 트리와 main 병합 결과 둘 다에서 한다.

## 허용 범위

- main 병합은 병합 검사가 통과하고 병합 SHA를 방에 먼저 게시한 뒤에 허용된다. 0.x는 리뷰어 2/2와 게이트 5개(`rust-ci-gate`, `web-ci-gate`, `install-ci-gate`, `documents-ci-gate`, `collab-engine-ci-gate`) PASS. 1.0.0 병합은 메인테이너 말이 있을 때 허용된다. 절차는 `fvoci-handoff`다.
- 브랜치 삭제는 main에 포함됐는지 다시 확인한 뒤 그 목록을 게시한 다음에 허용된다. 절차는 `fvoci-handoff`다.
- 리뷰 수는 여기에만 적는다. 작은 변경은 1명. `docs/rewrite.md`는 1명. workflow, xtask, 게이트, ruleset, `scripts/ci_selection.py`는 2명 ACCEPT. workflow·xtask·게이트의 0.x 병합은 그 2명 ACCEPT와 필수 CI PASS 뒤에, 병합 SHA와 규칙 요약을 게시한 다음 허용된다. AGENTS.md와 `.agents/` 스킬은 리뷰어 ACCEPT와 CI에 더해, 그 커밋 40자 SHA에 대한 메인테이너의 명시 승인이 있어야 한다(메인테이너, 2026-10-10 11:08 KST). GitHub ruleset·branch protection·권한 설정 변경은 문서 정책 변경과 구분한다. 리뷰어 2명 ACCEPT에 더해 정확한 대상과 변경 내용에 대한 메인테이너 명시 승인이 필요하며, 기존 보호 조건을 우회·약화하지 않는다. 주 리뷰어: AGENTS.md·`.agents/` 스킬은 리드가 배정하고 기본은 CI·웹과 Rust. xtask는 Rust가 주고 CI·웹이 교차. 의존성 변경은 Rust가 주고 DB가 교차.
- 작성자와 리뷰어는 다른 주체다. 리뷰어는 검토하는 커밋을 고치지 않는다.
- 태그와 릴리스는 메인테이너 말이 있고 대상 SHA가 방에 먼저 게시된 뒤에 허용된다. 이미 게시된 태그, `:0.y.z` 이미지, Release 파일은 그 내용 그대로 남을 때 유지된다.
- 배포, 시크릿, 패키지 공개 범위, 유료 사용은 메인테이너 말이 있을 때 허용된다.
- 저장소 배치는 같은 커밋에서 호출자를 고치면 바꿀 수 있다. 무료 티어만 쓴다.
- 클라우드 에이전트 추론 노력(메인테이너, 2026-10-10 10:28 KST): 작업마다 고른다. 기계적인 작업은 더 낮게 고른다.

## 손대지 말 것

- 게이트 workflow에 `paths:`가 없다. `ci-base-image.yml`의 이미지 경로 필터는 게이트 밖이다.
- 재실행은 전체 rerun이다. `gh run rerun --failed`는 범위 밖이다. `GITHUB_RUN_ATTEMPT` 생산자 대조는 유지한다. 머지 큐 근거는 게시한 큐 head와 `merge_group` checkout SHA = 그 main 커밋 SHA다.
- 통과는 타임아웃 증액·retry·sleep·skip 없이 나온다. regression·resolved·flaky 라벨에는 재현 근거가 있다.
- 원인이 알려진 flaky는 수정 없이 재실행하지 않는다. 같은 실패가 두 번이면 원인을 조사하고 고친다. 스타일·이름·절차에 대한 반복 리뷰는 2라운드까지다. 재현되거나 구체적인 호출 경로로 입증된 기능·API 계약 위반, 보안·fail-closed, 데이터 유실, 잘못된 저장 ACK, 복구 실패, 잘못된 영속 상태는 라운드와 관계없이 차단한다. 두 라운드 뒤에도 판단이 갈리면 근거와 미해결 쟁점을 메인테이너에게 전달하며 자동 ACCEPT로 바꾸지 않는다.
- 하네스(메인테이너, 2026-10-09 11:31 KST): 코드는 Rust(xtask)와 TypeScript(Bun)뿐이다. Python·shell 하네스 파일은 옮긴 뒤 제거한다. 이전은 intent부터다. diff·intent 표는 커밋 메시지와 PR 본문에만 있다.
- 메인테이너, 2026-10-10 10:39 KST. 경계는 2026-10-10 10:42 KST: 새 `.py`와 새 Python 코드는 없다. 이전 전까지 기존 Python 파일(예: `scripts/ci_selection.py`)의 기존 검사 수정은 허용된다. 새 기능이나 새 테스트 준비는 TypeScript 또는 Rust에 둔다.
- 외부 Python 도구(2026-10-09 repowise 결정): uv로 설치해 도구로 쓴다. 그 도구 때문에 레포에 Python 코드를 더하지 않는다.
- Bun/TS를 Bun에서 다시 띄울 때만 `process.execPath`를 쓴다. 그때 Playwright 자식 env에서는 `JEST_WORKER_ID`를 뺀다. Rust native helper·child는 그 바이너리로 띄우고 `process.execPath`로 대체하지 않는다.
- 타이밍·동시성에 기대거나 실제로 flaky였던 테스트만 기본 25회와 CPU 부하 5회를 통과한다.
- force push, `reset --hard`, 진행 중 CI 직접 취소, 운영 DB 변경은 허용 조건이 없다. 예외는 `draft/*` push의 자동 취소뿐이고 main·#347 push에는 없다. 그 run은 `CANCELLED(대체됨)`이라 판정 근거가 아니다.
- 시크릿·credential·접속 URL·host는 로그·커밋·보고·artifact 밖에 둔다. main 직접 push는 병합 허용 조건에 없다.

## 스폰

스킬의 기본 절차는 기본값이다. 완료 조건과 승인 경계를 지키면 다른 접근을 선택할 수 있다. 공통 파일은 저장소 사실·계약·완료 조건을 담고, 모델·추론 노력·역할별 보충 절차는 현재 실행 환경의 지원 설정이나 배정에 둔다. 모델별로 보안·데이터 보장을 달리하거나 공통 규칙을 복제하지 않는다.

스폰 프롬프트에는 결과, 제약과 허용 경로, 고정 base/head, 필요한 스킬, 검증 명령, 보고 형식, 멈출 지점을 적는다. 독립적인 작업만 병렬화하고 필수 독립 검토를 제외한 같은 조사·실행 검사를 여러 역할에 중복 배정하지 않는다. 탐색·검토는 지원되는 읽기 전용 권한을 우선하며 실제 제한이 없으면 제한했다고 주장하지 않는다. 에이전트 사이 메시지는 읽을 수 있는 문장으로 쓴다.
