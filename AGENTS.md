This codebase is yours: what it's for and how it's built are your call, and you'll keep building on it.

Storybook은 Cloudflare Workers Builds가 지어 올린다(Worker `fvoci-web-storybook`, 누구나 열고 색인은 막는다): main은 `storybook.fvoci.dev`, 다른 브랜치는 그 아래 미리 보기 주소. 설정은 `apps/web/wrangler.jsonc`, 명령은 `apps/web/package.json`의 `storybook:site`·`storybook:deploy`·`storybook:preview`, 머리글과 CSP는 `apps/web/tools/storybook/site.ts`, 고정한 Wrangler는 자기 잠금 파일과 함께 `apps/web/tools/storybook/wrangler/`.

## 스킬

- Rust를 쓰거나 고칠 때: `rust-best-practices` (`.agents/skills/rust-best-practices/SKILL.md`), 리뷰할 때: `rust-review` (`.agents/skills/rust-review/SKILL.md`)
- Svelte(`.svelte`, `.svelte.ts`)를 쓰거나 고칠 때: `svelte-code-writer` (`.agents/skills/svelte-code-writer/SKILL.md`), `svelte-core-bestpractices` (`.agents/skills/svelte-core-bestpractices/SKILL.md`)

## 코드 찾기

LSP 도구로 찾는다. 위치는 줄·글자 모두 1부터. 첫 질의가 "찾지 못함"이면 색인 중이니 한 번 더 묻는다.

- Rust(rust-analyzer): `incomingCalls`·`outgoingCalls`(부르는 쪽·불리는 쪽), `findReferences`, `goToImplementation`, `goToDefinition`, `workspaceSymbol`(이름으로).
- 웹: `.ts`의 함수·메서드를 누가 부르나는 `incomingCalls`(`.svelte`에서 부르는 곳까지 나온다). `.ts`에서의 `findReferences`는 정의 한 줄만 돌려주니 쓰지 않는다. `.svelte`에서는 `findReferences`와 `goToDefinition`.
- 기기마다 한 번: `bash .agents/setup-lsp.sh`.

## 검증

- `cargo xtask test`는 단위(`libs`, `doc`, 통합 시험 하나하나) 하나가 떨어져도 끝까지 돌고, 끝에 떨어진 시험과 그것만 다시 돌리는 줄(`cargo xtask test --database … --unit … -- --exact …`)을 찍는다. 다시 돌려 통과한 시험은 "flaky"로 남는다. 시간은 `target/xtask-times.jsonl`, 나눠 돌리기는 `--database <db> --part 2/4 --times <시간 파일>`. 웹 e2e는 `pnpm --filter web run e2e --last-failed`(`e2e:live`도 된다: first-use·desktop·sso의 실패는 그 셋을 통째로 다시 돈다; 다 통과한 뒤에는 "No tests found"로 끝난다). `FVOCI_TEST_CPUS=<수>`가 cargo test 스레드와 Vitest·Playwright 일꾼을 그 수의 기기처럼 맞춘다(`RUST_TEST_THREADS`·`--workers`가 있으면 그것이 이긴다).

## CI와 스택

- PR마다 `.github/workflows/ci.yml`이 `cargo xtask check`·`test`와 웹앱의 검증 전부를 GitHub의 실행기에서 같은 명령으로 돈다. main에서 갈라진 뒤 바뀐 파일(스택이면 그 층까지 모두)이 모두 `apps/web/`이면 Rust 시험을, 모두 `crates/`·`xtask/src/`의 `.rs`면 웹의 check·test·이야기·`e2e`·`e2e:apple`을 건너뛴다(`.github/scripts/plan.sh`). 볼 검사는 `ci-done` 하나다(작업 하나라도 실패·취소, 또는 그 규칙 밖의 건너뜀이면 빨강). 고치지 않은 곳의 시험으로 빨강이면 떨어진 작업만 한 번 다시 돌리고(`gh run rerun <실행> --failed`) 두 결과와 시험 이름을 보고한다. 묶음의 끝 검증은 PR의 `ci-done`이 갈음하고, 로컬에서는 고친 곳의 시험만 돌린다. `.github/`를 고친 PR이나 포크 PR은 합치기 전에 전체 diff를 직접 읽는다.
- 스택(`gh stack`)으로 올릴 때는 tracer-bullet 하나가 브랜치 하나, PR 하나다. 시작은 `gh stack init --base main fvoci/<묶음>-t1`, 올리기는 `gh stack submit --auto --open`(새 PR)과 `gh stack push`, 다음 세션은 `gh stack view`, `gh stack sync`(아래 층이 합쳐졌거나 GitHub에서 rebase됐으면 맞춘다), `gh stack top` 뒤에 `gh stack add fvoci/<묶음>-t2`(브랜치 `fvoci/<묶음>`이 있으면 `fvoci/<묶음>/t1`은 만들 수 없다). 아래 브랜치를 고치면 그 브랜치에 새 커밋을 얹고 `gh stack rebase --upstack`, `gh stack push`. main이 앞서면 `gh stack rebase`, `gh stack push`(합치기가 아니다). 모든 워크트리가 한 저장소를 함께 쓴다: 상태와 잠금은 공통 git 디렉터리(`git rev-parse --git-common-dir`)의 `gh-stack*`이고 트렁크는 로컬 `main`이다. `git rev-list --count origin/main..main`가 0이 아니면(합친 것이 아직 올라가지 않음) `init`·`rebase`·`sync`를 하지 않는다: 그 커밋이 PR에 든다. 로컬 `main`이 뒤처져 있으면 `rebase`·`sync`가 main 워크트리를(깨끗하면) 앞으로 당긴다. 충돌로 멈춘 `gh stack rebase`는 그것을 연 세션이 그 자리에서 `--continue`나 `--abort`로 끝낸다. 그동안 다른 워크트리의 변경 명령은 거절된다: 거절된 쪽은 남의 rebase를 `--abort`하지 않고 기다리거나 조율 세션에 알린다. `rerere`는 켜지 않는다(설정이 모든 워크트리에 걸린다).
