# FVOCI

Rust 서버(Tokio, axum 0.8, SQLx, Serde, tracing), UI는 Vue 3 + Nuxt UI + Vite, TypeScript는 Bun(`.bun-version`). DB는 PostgreSQL·로컬 SQLite·원격 libSQL/Turso. 협업 문서는 Yrs. 범위·수락 `docs/rewrite.md`, 설치·실행 `RUNNING.md`, 발행 `docs/RELEASING.md`(사람만 명시 시작). 수락 정본은 PR #347 체크포인트 `<!-- fvoci-current-checkpoint -->`와 #331. 레포 문서에 현재 head SHA·run ID·세션·로컬 경로를 적지 않는다.

## 어디를 보나

- 스키마: `migrations/postgres/060`, `migrations/sqlite/060`, `src/db/migrate.rs`
- CI 선택: `scripts/ci_selection.py`. 게이트: `.github/workflows/{rust,web,install,documents,collab-engine}.yml`
- 웹 정적 검사: `.github/workflows/web.yml` `web-static`, 루트 `package.json`. Turso: `docs/testing-turso.md`
- 작업별 완료 조건: `.agents/skills/*/SKILL.md` (description 첫 문장이 트리거)

## 완료 조건

경로가 바뀌면 그 명령이 exit 0이다.

- `src/`, `crates/`, `migrations/`: `cargo fmt --check`와 그 타깃의 `cargo test --locked --offline`
- `apps/web/`, `packages/` TS·Vue: `bun run lint`, `bun run format:check`, 해당 패키지 `bun --bun run typecheck`
- `.github/workflows/`, `scripts/ci_selection.py`: `bash scripts/test-ci-selection.sh`
- 인가·RLS·잠금·원자성: 실제 DB·앱 역할. `TEST_DATABASE_URL`이 없으면 실패로 남긴다.

되돌릴 수 있고 영향이 작은 변경에는 구현을 그대로 반영하는 테스트를 새로 쓰지 말고, 변경에 맞는 테스트와 필수 검사가 통과하면 새 문제가 없는 한 테스트를 넓히지 마라.

그 SHA의 결과는 PASS, FAIL, NOTRUN, MISSING이다. CANCELLED·SKIP은 PASS가 아니다. 이전 SHA를 현재 head에 합치지 않는다. 리뷰 판정은 원격 CI를 대신하지 않는다.

## 허용 범위

- main 병합은 병합 검사가 통과하고 병합 SHA를 방에 먼저 게시한 뒤에 허용된다. 0.x 검사는 리뷰어 2/2 ACCEPT와 필수 게이트 5개(`rust-ci-gate`, `web-ci-gate`, `install-ci-gate`, `documents-ci-gate`, `collab-engine-ci-gate`) PASS다. 확인 순서는 `fvoci-handoff` 완료 조건이다. 1.0.0 병합은 영환님 말이 있을 때 허용된다.
- 브랜치 삭제는 main에 포함됐는지 다시 확인한 뒤 그 목록을 게시한 다음에 허용된다.
- ruleset 변경은 리뷰어 2명 ACCEPT 뒤에 허용된다. 작은 커밋은 리뷰어 1명. `workflows`, `xtask`, `AGENTS.md`, `.agents/`, 게이트, `scripts/ci_selection.py`는 리뷰어 2명. `docs/rewrite.md`는 리뷰어 1명.
- 작성자와 리뷰어는 다른 주체다. 리뷰어는 검토하는 커밋을 고치지 않는다.
- 태그와 릴리스는 영환님 말이 있고 대상 SHA가 방에 먼저 게시된 뒤에 허용된다. 이미 게시된 태그, `:0.y.z` 이미지, Release 파일은 그 내용 그대로 남을 때 유지된다.
- 배포, 시크릿, 패키지 공개 범위, 유료 사용은 영환님 말이 있을 때 허용된다.
- 저장소 배치는 같은 커밋에서 호출자를 고치면 바꿀 수 있다. 무료 티어만 쓴다.

## 손대지 말 것

- 게이트 workflow에 `paths:`가 없다. `ci-base-image.yml`의 이미지 경로 필터는 게이트 밖이다.
- 재실행은 전체 rerun이다. `gh run rerun --failed`는 범위 밖이다. `GITHUB_RUN_ATTEMPT` 생산자 대조는 유지한다.
- 머지 큐 근거는 게시한 큐 head와, `merge_group` checkout SHA = 그 main 커밋 SHA다.
- 통과는 타임아웃 증액·retry·sleep·skip 없이 나온다. regression·resolved·flaky 라벨에는 재현 근거가 있다.
- 원인이 알려진 flaky는 재실행하지 않는다. 같은 실패가 두 번이면 고친다. REJECT는 2라운드까지고, 3라운드부터 막는 것은 보안·fail-closed뿐이다.
- 하네스(영환님, 2026-10-09 11:31 KST): 하네스 코드는 Rust(xtask)와 TypeScript(Bun)뿐이다. Python·shell 하네스 파일은 옮긴 뒤 제거한다. 파일이 옮겨지기 전까지 기존 Python 파일은 로직을 포함해 고칠 수 있다. 새 `.py` 파일은 만들지 않는다. 이전은 intent부터다. diff·intent 표는 커밋 메시지와 PR 본문에만 있다.
- 외부 Python 도구(2026-10-09 repowise 결정): uv로 설치해 도구로 쓸 수 있다. 그 도구 때문에 레포에 Python 코드를 더하지 않는다.
- 자식 프로세스는 `process.execPath`로 띄운다. 새 테스트는 기본 25회와 CPU 부하 5회를 통과한다.
- force push, `reset --hard`, 진행 중 CI 직접 취소, 운영 DB 변경은 허용 조건이 없다. 예외는 `draft/*` push의 자동 취소뿐이고 main·#347 push에는 없다. 그 run은 `CANCELLED(대체됨)`이라 판정 근거가 아니다.
- 시크릿·credential·접속 URL·host는 로그·커밋·보고·artifact 밖에 둔다. main 직접 push는 병합 허용 조건에 없다.

## 스폰

스폰 프롬프트에는 결과, 제약, 검증 명령, 멈출 지점을 적는다. 에이전트 사이 메시지는 읽을 수 있는 문장으로 쓴다. 지시문은 목표, 허용 경로, 고정 base/head, 검증, 수락, 중단만 적는다.
