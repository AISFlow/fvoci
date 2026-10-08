# FVOCI 기여 안내

개발 환경 준비, 변경 종류별 검사, 실패 진단과 PR 절차를 정리합니다. 설치·운영 설정은 [RUNNING.md](RUNNING.md),
현재 범위는 [docs/rewrite.md](docs/rewrite.md), AI 에이전트 규칙은 [AGENTS.md](AGENTS.md)를 봅니다.

## 저장소 구성

| 경로                            | 내용                                                        |
| ------------------------------- | ----------------------------------------------------------- |
| `src/`, `migrations/`, `tests/` | Rust 서버(`fvoci-server`, `fvoci-migrate` 등)와 통합 테스트 |
| `crates/`                       | 협업 엔진·문서 추출 helper 등 별도 crate                    |
| `apps/web/`, `packages/`        | Vue 3 웹 앱, 편집기, i18n (Bun workspace)                   |
| `scripts/`                      | 테스트 wrapper·CI 도구·설치/백업 smoke                      |
| `.github/workflows/`            | CI. 각 job의 명령이 검사의 정본입니다                       |
| `infra/rust/`                   | 컨테이너 이미지와 Compose 설치 파일                         |

## 준비물

| 도구                 | 버전·출처                                                                                                          | 필요한 작업                         |
| -------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------- |
| rustup               | `rust-toolchain.toml`이 1.98.1을 고정합니다. `rustfmt`·`clippy`는 직접 추가합니다                                  | 모든 Rust 작업                      |
| C 컴파일러, libclang | 고정 SQLite 빌드와 `libsqlite3-sys` binding 생성에 씁니다. Ubuntu 예: `build-essential`, `curl`, `libclang-18-dev` | Rust 빌드                           |
| Bun                  | `.bun-version`(1.4.2). Node는 필요하지 않습니다                                                                    | 웹 빌드·lint·단위 테스트            |
| Python, uv           | Python 3.11 이상, [uv](https://docs.astral.sh/uv/)                                                                 | `scripts/`의 도구와 그 검사         |
| Docker               | 로컬 PostgreSQL·Meilisearch·MinIO 컨테이너                                                                         | DB 테스트, 브라우저 e2e, 설치 smoke |

처음 한 번 실행합니다.

```sh
rustup component add rustfmt clippy
cargo fetch --locked
cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml   # 협업 helper는 별도 lockfile
bun ci
uv sync
```

### 고정 SQLite 준비

서버는 SQLite 3.53.4와 고정 source ID만 허용합니다(`src/db/pool.rs`). 시스템 SQLite로도 `cargo check`·clippy는
되지만, SQLite를 여는 테스트는 버전 확인에서 실패합니다. CI와 컨테이너 이미지처럼 `scripts/prepare-sqlite-ci.sh`로
검증된 SQLite를 한 번 빌드하고, Rust 명령을 실행할 셸마다 그 환경을 불러옵니다. `curl`(sqlite.org에서 내려받기)과
`libclang.so`가 있는 `LIBCLANG_PATH`가 필요합니다.

```sh
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
mkdir -p target/sqlite
bash scripts/prepare-sqlite-ci.sh --parent "$PWD/target/sqlite" --env-file target/sqlite/env.sh
. target/sqlite/env.sh   # 새 셸마다 다시 불러옵니다
```

`--parent`나 불러온 환경 없이 `prepare-sqlite-ci.sh`를 실행하면 호출마다 새 임시 디렉터리에 SQLite를 다시 빌드하고,
바뀐 경로 때문에 `libsqlite3-sys`와 서버 crate도 다시 컴파일합니다. 위 환경을 불러온 셸에서는
`scripts/run-db-tests.sh`처럼 이 스크립트를 다시 거치는 명령도 같은 디렉터리를 재사용합니다.

## 변경별 빠른 검사

아래 명령은 외부 서비스 없이 실행됩니다. 바꾼 영역의 검사만 고르고, 공통 인가·DB·CI를 바꿨다면 범위를 넓힙니다.
CI가 실제로 실행하는 전체 목록은 workflow 파일이 정본입니다. 이 문서는 그중 로컬에서 먼저 돌릴 시작점만 둡니다.

**Rust 서버** ([`rust.yml`](.github/workflows/rust.yml)의 `fast` job, 고정 SQLite 환경을 불러온 셸)

```sh
cargo fmt --check
cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings
cargo test --locked --offline --lib --bin fvoci-server
```

SQLite 작업 텍스트 비교 테스트(`db::tasks`)는 CI runner인 Ubuntu 26.04 x86_64의 glibc 2.43과 `en_US.UTF-8`
`locale-archive` 해시에 맞춰 검증된 profile(`src/db/task_scalar_pg18_profile.json`)만 허용합니다. 다른 glibc나
locale archive에서는 `Task GNU locale …` 오류로 실패하며, 이는 우회하지 않고 CI에서 확인합니다.

**웹·편집기** ([`web.yml`](.github/workflows/web.yml)의 `web-static`·`web-checks` job)

```sh
bun run lint
bun run format:check
(cd apps/web && bun --bun run typecheck && bun run test)
(cd packages/editor && bun --bun run typecheck && bun run test)
```

ESLint·Prettier 설정이나 도구 버전을 바꿀 때는 `bun run lint:fixtures`도 실행합니다(수 분 소요).
세부 규칙은 [scripts/WEB_LINT.md](scripts/WEB_LINT.md)에 있습니다.

**Python 도구** (`scripts/`)

```sh
uv run ruff check
uv run mypy
```

workflow나 CI 선택 규칙을 바꿨다면 CI의 `ci-plan`과 같은 검사를 실행합니다. PyYAML 버전은
`scripts/ci_selection_requirements.txt` 한 곳에서 고정합니다.

```sh
uv run --with-requirements scripts/ci_selection_requirements.txt bash scripts/test-ci-selection.sh
```

e2e wrapper(`scripts/web-e2e-*`, `scripts/run-web-e2e.sh`)를 바꿨다면 `bash scripts/test-web-e2e-groups.sh`를 실행합니다.

## DB 통합 테스트 (Docker 필요)

`scripts/start-test-postgres.sh`는 실행마다 새 PostgreSQL 컨테이너(127.0.0.1의 임의 포트)를 띄우고,
`TEST_DATABASE_URL`을 넘긴 명령이 끝나면 컨테이너를 지웁니다. 고정 SQLite 환경을 불러온 셸에서 실행합니다.
명령을 생략하면 `scripts/run-db-tests.sh`가 `db_integration` 한 target만 실행합니다.

```sh
bash scripts/start-test-postgres.sh   # db_integration
bash scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test <target>
FVOCI_TEST_PG_MAJOR=16 bash scripts/start-test-postgres.sh cargo test --locked --offline --features db-tests --test <target>
```

- `--features db-tests`를 항상 붙입니다. 일부 target은 feature 없이 실행하면 테스트 0개로 성공합니다.
- 협업 세션을 쓰는 target(예: `revision_integration`)은 협업 helper가 필요합니다. 한 번 빌드하면 테스트가
  `crates/collab-engine/target/debug/collab-engine`을 찾습니다.
  `(cd crates/collab-engine && cargo build --locked --offline --bin collab-engine --features worker)`
- CI가 실행하는 target 목록과 PostgreSQL 버전(16·17·18) 조합은 `rust.yml`의 `postgres` job,
  협업 target 묶음은 `scripts/run-rust-collaboration-ci-tests.sh`가 정본입니다.
- 검색 테스트는 `scripts/start-test-meili.sh`, S3 테스트는 `scripts/start-test-minio.sh`가 같은 방식으로 명령을 감쌉니다.

## 브라우저 e2e (Docker, Chromium 필요)

```sh
bash scripts/prepare-web-e2e.sh                   # 의존성과 Playwright가 고정한 Chromium 리비전 준비
bash scripts/run-web-e2e.sh e2e/<spec>.spec.ts    # 서버·웹 빌드 후 해당 그룹 실행
```

미리 설치된 다른 리비전의 브라우저는 쓰지 않습니다. 설치 위치는 `PLAYWRIGHT_BROWSERS_PATH`로 바꿀 수 있습니다.
`run-web-e2e.sh`는 API 계약을 다시 생성하고 웹·서버·e2e fixture를 빌드한 뒤, 인자로 준 spec들을 한 그룹(새
PostgreSQL·Meilisearch·서버 한 벌)으로 실행합니다. CI는 같은 wrapper를 `--ci-shard N`(8개 shard)으로 실행하며
spec 파일마다 그룹을 나눕니다(`scripts/web-e2e-groups.py`). `workspace-wiki-flow`는 `workspace-flow`가 만든 관리자로
로그인하므로 두 spec을 함께 실행합니다.

## 실패 진단

- **브라우저 e2e**: 실패한 그룹은 `retained failure artifacts for group <이름> in <디렉터리>`를 출력합니다.
  그 디렉터리에 `server.log`(접속 정보 가림), `playwright-output/**/error-context.md`, `browser-summary.txt`,
  `net-events.log`가 남습니다. CI에서는 같은 내용이 `*-browser-failure-*` artifact로 올라갑니다.
- **서버가 준비되지 않음**: e2e는 Playwright를 시작하기 전에 마지막 `/api/v1/setup` 상태와 서버 로그를 출력하고 실패합니다.
- **Rust 테스트 하나만**: `cargo test --locked --offline --features db-tests --test <target> <이름> -- --exact --nocapture`.
- **CI**: `*-ci-gate` job은 선택된 job이 모두 `success`이고 선택되지 않은 job이 `skipped`일 때만 통과합니다.
  gate 실패는 결과일 뿐이므로 먼저 실패한 job의 첫 오류를 봅니다. 실패를 재시도·skip·timeout 증가로 숨기지 않습니다.

## PR 절차

1. 기본 브랜치에서 작업 브랜치를 만들고, 서로 독립적인 변경은 별도 커밋으로 나눕니다.
2. 위의 해당 검사를 로컬에서 실행하고, PR 본문에 실행한 명령과 결과·실행하지 못한 검사를 적습니다.
3. 새 의존성은 필요성·대안·버전·라이선스·빌드 영향을 함께 설명합니다. lockfile은 도구로만 갱신합니다.
4. 비밀값(`.env`, 토큰, 접속 URL)을 커밋하거나 로그·PR에 붙이지 않습니다.
5. CI의 필수 job과 리뷰가 통과해야 병합됩니다. 병합·태그·릴리스는 [docs/RELEASING.md](docs/RELEASING.md)의 별도 절차입니다.
