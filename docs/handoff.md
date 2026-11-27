# 서버 개발 안내

## 저장소

서버는 저장소 루트에(`crates/`, `xtask/`, `contracts/`, `docs/`), 웹앱은 `apps/web/`에 있다. 이 문서는 서버를 고치는 사람을 위한 안내이고, 웹앱의 것은 `apps/web/docs/handoff.md`다.

- **브랜치와 푸시.** 일마다 `main`에서 브랜치(또는 워크트리)를 갈라 일하고, PR로 `main`에 합친다. 강제 푸시와 올라간 커밋 고쳐 쓰기(`amend`·`rebase`·`reset`)는 하지 않고, 고칠 것은 새 커밋으로 얹는다: 스택의 브랜치를 `gh stack`(또는 GitHub의 Rebase stack)이 rebase해 `--force-with-lease`로 올리는 것만 예외다(`AGENTS.md`의 "CI와 스택").
- **검증 명령.** 양쪽 모두 저장소 루트에서 돌린다.

  | 쪽 | 명령 | 명령의 설명과 기대 결과 |
  |---|---|---|
  | 서버 | `cargo xtask check`, `cargo xtask test` | 이 문서의 1절 |
  | 웹앱 | `pnpm install --frozen-lockfile` 뒤에 `pnpm --filter web run check`, `test`, `build`, `e2e` | `apps/web/docs/notes.md`의 "검증 명령". `pnpm`을 놓는 법은 `apps/web/docs/handoff.md`의 "도구" |

- **무시 규칙.** 저장소 루트의 `.gitignore`가 `target/`, `node_modules/`를 무시한다. 웹앱의 빌드 산출물과 글꼴은 `apps/web/.gitignore`가 맡는다.
- **컨테이너.** `cargo xtask test`가 PostgreSQL과 S3 컨테이너를 스스로 준비하고, 끝날 때 멈춘다(2절). 웹앱의 실서버 검증에도 PostgreSQL이 필요하다.
- **설정.** 키 전부는 `docs/configuration.md`에 있고, `config.rs`의 테스트가 문서와 코드(키, 기본값, 환경 변수)를 견준다. 키를 더하면 같은 커밋에서 문서에 넣는다. 예시 파일 `docker/fvoci/config.json`은 테스트가 읽고 검사한다(읽지 않는 키가 없어야 한다). 키는 적힌 그대로만 맞고, 읽지 않는 키는 시작할 때 이름만 경고한다. 값은 서버가 나열한 것뿐이다(`database_kind`는 `sqlite`·`postgres`, `file_storage`는 `local`·`s3`). `metrics_address`는 지표를 따로 연 리스너에서 낸다(`fvoci/metrics`). S3의 키 가운데 자격이 아닌 것은 `FVOCI_FILE_STORAGE_S3_*` 환경 변수로도 받고, 자격은 `AWS_*`로만 받는다.
- **내장 템플릿은 `crates/server/assets/templates/` 한 곳에 있다.** 일곱 개가 한국어와 영어로 한 파일씩이고, 본문이 보이는 그림과 문서는 그 아래 `files/`에 있다. 서버가 바이너리에 넣는다. 글은 그 파일에서 고친다(그 폴더의 `README.md`). 웹앱의 메모리 구현은 제 폴더(`apps/web/src/lib/state/memory/seed/`)의 보드로 시작한다.

| 알고 싶은 것 | 있는 곳 |
|---|---|
| API의 계약 | `contracts/openapi.json`, WebSocket은 `contracts/websocket.md`·`websocket.json` |
| 설정의 키 | `docs/configuration.md` |
| CDN과 프록시 뒤에 둘 때 | `docs/fvoci-cdn.md` |
| 웹앱의 구조, 날짜 모델 | `apps/web/docs/notes.md` |

## 1. 검증

```bash
cargo xtask check    # 포맷, clippy, 단위 테스트, 문서, 이름, 층, 길이, 계약, 라이선스·권고. 외부 서비스 없음
cargo xtask test     # 컨테이너 준비 → SQLite 전체 → PostgreSQL(store, server) → S3 저장소 테스트
cargo xtask openapi  # contracts/openapi.json 이 서버가 내는 것과 다르면 실패. --write 가 고쳐 쓴다
```

**기대 결과.**

| 무엇 | 결과 |
|---|---|
| `cargo xtask check` | 아홉 단계 PASS. 층의 규칙은 `xtask/src/layers.rs`, 길이의 한도는 `xtask/src/lengths.rs`, 이름은 `xtask/src/names.rs`·`banned.rs`에 있다. 예외 목록은 없다: 규칙에 걸리는 코드는 나누거나 옮겨서 고친다 |
| `cargo xtask test` | 실패 0. 수는 커밋마다 달라지니 실행 결과를 본다 |

- 건너뛴 단계는 요약에 `SKIP`으로 나온다(`--skip-licenses`, `--database`). `SKIP`이 있는 실행, `--part`·`--unit`으로 나눠 돈 실행을 전체 통과로 보지 않는다.
- `cargo xtask test`는 `FVOCI_REGRESSION_UPDATE`가 설정돼 있으면 시작하지 않는다(아래의 갱신 절차).
- 환경 변수로 다른 PostgreSQL·S3·컨테이너 이름·포트를 주는 법은 `xtask/src/test.rs`와 `xtask/src/containers.rs`의 머리 주석에 있다. PostgreSQL을 직접 줄 때는 슈퍼유저가 아닌 역할의 주소와 함께 슈퍼유저의 주소(`FVOCI_STORE_TEST_SUPERUSER_CONN_STRING`)를 준다(조직 사이의 벽은 슈퍼유저에게 걸리지 않는다. 서버와 저장소가 그런 역할을 어떻게 다루는지 보는 테스트만 그 주소를 쓴다).

**걸리는 시간.** `cargo xtask check`와 `test`가 실행마다 단계·빌드·단위의 시간을 `target/xtask-times.jsonl`에 한 줄씩 더한다(`jq -s . target/xtask-times.jsonl`).

**커밋마다 clippy를 통과시킨다.** 워크스페이스 린트가 엄격해서(`too_many_lines`, `as_conversions`, `print_stderr`, `similar_names` 등) 테스트 코드에서 자주 걸린다. `cargo doc`도 경고가 오류다. 없는 항목을 가리키는 문서 링크 하나로 check가 실패한다.

### 회귀 스냅샷을 갱신하는 절차

스냅샷은 `crates/server/tests/regression/golden/`에 있고 SQLite와 PostgreSQL이 같은 파일을 쓴다. 응답을 의도해서 바꿨을 때만 갱신한다.

1. 갱신하기 **전에** 비교로 한 번 돌려 무엇이 어긋나는지 읽는다: `cargo test -p fvoci-server --test regression`. 실패 메시지가 단계 이름, 요청, 다른 자리를 보여 준다. 바꾸려던 것만 어긋났는지 여기서 본다.
2. 기록한다: `FVOCI_REGRESSION_UPDATE=1 cargo test -p fvoci-server --test regression`. 다시 쓴 파일마다 `recorded <파일>: CHANGED|unchanged|NEW (not compared)` 한 줄이 나온다. 이 실행은 무엇을 받든 통과하므로 검증이 아니다.
3. `git diff -- crates/server/tests/regression/golden`을 **읽는다.**
4. 변수 없이 다시 돌려 통과를 확인하고, PostgreSQL에서도 돌린다(`cargo xtask test`). 한 DB에서 기록한 것이 다른 DB에서 어긋나면 그 응답의 순서나 값이 DB에 달린 것이다.
5. 코드와 **같은 커밋**에 넣고, 무엇이 왜 바뀌었는지 커밋 메시지에 적는다.

주의:

- 변수는 정확히 `1`이어야 한다. 다른 값(`0`, `true`)은 오류이고 `CI`에서는 기록을 거부한다. 셸에 `export`해 두지 않는다.
- 하네스는 목록을 **온 순서대로** 기록한다. 목록을 돌려주는 라우트마다 순서가 정해져 있고(계약의 라우트 설명) 스냅샷이 그 순서를 고정한다.
- 새 목록 라우트를 만들면 스토어의 쿼리에 순서를 넣고 `crates/store/tests/store/order.rs`에 테스트를 더한다. 순서 없는 목록은 두 DB에서 스냅샷이 어긋나는 것으로 드러난다.
- 타임스탬프는 모두 `<ts>`, ID는 `<id:N>`, `requestId`는 `<request-id>`로 바뀐다. 응답 본문을 **문자열로** 기록하는 관찰 단계는 이 정규화를 지나지 않으니, 관찰 단계는 JSON으로 기록한다.
- 권한 표의 준비 요청은 성공해야 한다. 실패하면 단언으로 멈춘다. 칸이 엉뚱한 이유의 상태 코드를 기록하지 않게 한 것이다.
- 없는 라우트(채널, 통계, 컴플라이언스, 메타데이터)의 단계와 권한 표의 행은 404를 고정한다.

### 계약을 고치는 절차

`contracts/openapi.json`은 손으로 고치지 않는다. 핸들러의 `#[utoipa::path]`와 문서 주석, 와이어 타입, `api/contract.rs`의 표에서 나온다.

1. 라우트를 더하거나 고친다. 핸들러에 `#[utoipa::path(...)]`를 달고 `routes()`에 `routes!(핸들러)`로 넣는다. 주석 없는 핸들러는 `routes!`가 받지 않는다.
2. 새 스키마가 생기면 `api/contract.rs`의 `SCHEMAS`에 늘 있는 멤버를 적는다. 빠뜨리면 문서를 만드는 일이 실패하고 무엇이 빠졌는지 말한다.
3. 요청의 모양이 답과 다르면 `api/contract/bodies.rs`에 문서 전용 타입을 둔다. 해석하는 타입과 맞는지는 사람이 본다(검사가 없다).
4. 목록을 답하면 순서를 설명에 적고 `contract.rs`의 테스트 표 `ORDERS`에 넣는다. 빠뜨리면 `every_list_says_its_order`가 실패한다.
5. `cargo xtask openapi --write`로 파일을 다시 쓰고, `git diff contracts/`를 읽는다. 웹앱의 타입도 같은 커밋에서 다시 만든다: `pnpm --filter web run api:types`(`apps/web/src/lib/api/contract.ts`).

### 스키마를 고치는 절차

이주는 `crates/store/src/schema/steps.rs`의 `STEPS`이고, 첫째가 기준선(`schema/<종류>/0001_baseline.sql`)이다.

1. 새 이주는 다음 번호로 두 종류의 SQL을 함께 덧붙인다. 올라간 이주는 고치지 않는다: 체크섬이 달라진 데이터베이스는 열리지 않는다.
2. 조직의 행을 건드리는 이주는 `fvoci.system`을 켜는 줄로 시작한다(`schema.rs`의 시험이 본다).
3. 기준선 전의 기록을 기준선의 것으로 바꾸는 일은 `schema::re_anchor`가 하고, 그 기록은 `schema/pre_baseline.rs`에 있다. 시험은 `crates/store/tests/store/before_baseline.rs`(두 DB)다.

### xtask에 넣지 않은 것

```bash
target/tools/bin/cargo-about generate --manifest-path crates/server/Cargo.toml -o NOTICE about.hbs   # 의존성이 바뀌었을 때
cargo xtask docker   # 이미지 둘(서버+웹앱, 검색 서버)을 빌드해 docker/compose.yaml로 띄워 가입~검색을 시험하고 내린다. GHCR에 올리는 명령은 --push-plan
```

- 의존성이 바뀐 커밋마다 `NOTICE`를 다시 만든다. `zstd-sys`가 넣는 C 라이브러리의 BSD 고지는 cargo-about이 보지 못해 `about.hbs`에 손으로 적었다.
- 이미지의 쓰는 법은 `docker/fvoci/Dockerfile`, `docker/search/Dockerfile`, `docker/compose.yaml`의 머리 주석에 있다.
- `cargo xtask check`의 마지막 단계는 `cargo-deny`가 있어야 돈다. 워크트리의 `target/tools/bin`이나 검색 경로에서 찾는다. 워크트리에 두는 명령: `CARGO_TARGET_DIR=$PWD/target/tools/build cargo install cargo-deny --locked --root $PWD/target/tools`.

## 2. 시험 컨테이너와 빌드

**평소에 떠 있는 컨테이너는 없다.** `cargo xtask test`가 둘을 만들고(PostgreSQL은 `postgres:18`), 테스트 실행이 시작하거나 만든 것은 그것을 쓰는 마지막 실행이 끝날 때 멈춘다. 멈춘 채 남아 있는 것은 다음 `cargo xtask test`가 다시 시작한다.

- 한 기기의 워크트리들이 컨테이너를 함께 쓴다. 테스트 실행이 시작하거나 만든 컨테이너는 그것을 쓰는 마지막 실행이 끝날 때(통과, 실패, Ctrl-C 모두) 멈춘다. 다른 실행이 아직 쓰는 동안에는 멈추지 않는다. 테스트 실행이 띄우지 않은 것(손으로 띄운 것)은 그대로 둔다.
- 실행들은 시스템 임시 폴더의 `fvoci-xtask-containers/`에서 만난다. 컨테이너마다 쓰는 동안 잡는 잠금, 시작하고 멈추는 동안 잡는 잠금, 테스트 실행이 시작했다는 표시(`<이름>.started`)가 있다. 표시는 멈출 때 지운다.
- Ctrl-C를 받으면 돌던 단계가 끝나기를 기다린 뒤 나머지를 건너뛰고, 컨테이너를 놓는다. 컨테이너를 기다리던 중이면 기다림을 그만둔다. 그 사이 다시 Ctrl-C를 눌러도 같고, `docker stop`은 Ctrl-C를 받지 않는 제 프로세스 그룹에서 돈다.
- 터미널이 아니라 xtask 프로세스에만 보낸 `SIGTERM`은 돌던 `cargo test`에 닿지 않는다. xtask는 그 단계가 끝나기를 기다린다. `SIGKILL`로 죽이면 컨테이너가 떠 있는 채 남는다. 표시가 남아 있으므로, 그 컨테이너를 쓰는 다음 실행이 끝날 때 멈춘다.

`cargo xtask test`가 만드는 컨테이너는 둘이다. 값은 모두 테스트 전용이다.

| 컨테이너 | 주소 | 접속 |
|---|---|---|
| `fvoci-test-postgres` (PostgreSQL) | `127.0.0.1:15434` | 테스트는 슈퍼유저가 아닌 역할 `fvoci`로 붙는다(xtask가 만든다). 이미지의 슈퍼유저 `postgres`로는 그 역할과 표의 주인을 다루는 테스트만 붙는다. 비밀번호 `fvoci-test`, DB `fvoci` |
| `fvoci-test-s3` (S3 호환) | `127.0.0.1:18333` | 인증 없음, 버킷 `fvoci-test` |

- `cargo xtask test`가 멈춘 컨테이너는 시작하고 없는 것은 만든다. 재시작 정책은 없다.
- 같은 이름의 컨테이너가 다른 포트에 게시돼 있으면 xtask가 거절한다. 여러 묶음을 나란히 돌릴 때는 `FVOCI_TEST_PG_CONTAINER`·`FVOCI_TEST_PG_PORT`·`FVOCI_TEST_S3_CONTAINER`·`FVOCI_TEST_S3_PORT`를 묶음마다 다르게 준다.
- 패닉으로 끝난 테스트는 PostgreSQL에 `t_*` 스키마를 남긴다. 해는 없다. 컨테이너를 지우면 함께 없어진다.
- `/tmp`가 RAM인 기기에서는 빌드 산출물과 큰 파일을 워크트리의 `target/` 아래에 둔다.
- **빌드 크기.** `[profile.dev]`의 `debug = "line-tables-only"`: 디버그 정보 전부보다 빌드가 빠르고 `target`과 서버 바이너리가 절반 아래다. 줄 번호는 남는다.

## 3. 걸렸던 함정

요청 본문과 한도

- **413을 바로 답하면 클라이언트가 못 받는다.** 본문을 쓰는 중에 연결이 닫혀 broken pipe가 된다. 그래서 거절한 본문을 16 MiB·5초까지 읽어 버린 뒤 답한다(`api/limits.rs`의 `DISCARD_LIMIT`, `discard`). 업로드 한도를 넣을 때 같은 일이 생긴다. 단, `Expect: 100-continue`가 있으면 읽지 않는다(읽는 것이 "보내라"가 된다).
- **axum의 추출기 한도(2 MB)는 자기 형식의 평문 413을 낸다.** 우리 한도 아래에서 먼저 걸린다. 그래서 라우터 묶음마다 `DefaultBodyLimit::disable()`과 우리 미들웨어를 함께 둔다. 새 라우터 묶음을 만들면(관리 소켓도) 둘 다 붙인다.
- **한도 미들웨어는 세션 검사보다 먼저 돈다.** 그래서 토큰이 없는 요청은 어디서나 64 KiB다. 토큰이 있는데 본문이 64 KiB를 넘으면 미들웨어가 세션을 먼저 확인한다. 없는 세션이면 본문을 읽지 않고 401이다. 본문이 없는 라우트에 본문을 보내면 한도까지는 읽는다.

오류 응답과 404 규칙

- **새 라우트는 `tests/api/visibility.rs`의 표에 넣는다.** "숨겨진 것"과 "없는 것"의 답이 상태·코드·문구까지 같아야 한다. 문구에 요청의 ID를 되풀이하면 표의 비교가 실패한다.
- **친절한 409가 구멍이 된다.** 클라이언트가 고른 ID가 겹친다고 409를 답하면, 그 답이 남의 것의 ID가 있다는 것을 알려 준다. 그래서 서버가 ID를 만들고, ID를 적은 요청은 그 ID가 있든 없든 같은 답으로 거절한다. 클라이언트가 ID를 정하는 자리는 모두 같은 문제다.
- **저장소의 실패를 "없음"이나 "권한 없음"으로 읽지 않는다.** `App::container_access`는 `Result<Option<_>>`이다. `.is_ok()`, `Err(_) =>`로 뭉개면 DB가 잠깐 실패할 때 모든 보드가 404가 된다.
- **`AppError::context`는 로그로만 간다.** 4xx의 문구는 바뀌지 않는다. 클라이언트에게 할 말은 생성자에 적는다.
- **레코드 본문은 `decode_object`로 읽는다.** `decode_json`은 배열을 멤버 순서대로 읽어 버린다(`[]`가 빈 패치가 된다). 목록이 본문인 라우트만 `decode_json`이다.

작업 방식

- **임시 파일은 파일 하나만 지운다.** 디렉터리를 통째로 지우면 커밋된 파일이 함께 지워진다(`git checkout HEAD -- <경로>`로 되돌린다). 시험용 파일은 `target/` 아래에 둔다.
- **여러 파일을 고치는 스크립트는 실패한 자리에서 멈춘다.** 앞의 파일은 써졌고 뒤는 안 써진 상태가 된다. 실패 뒤에는 어느 편집이 들어갔는지 다시 본다. `cargo fmt`가 줄을 다시 나누므로 찾을 문자열은 포맷한 뒤의 글에서 잡는다.
- **`git stash`는 쓰지 않는다.** 스택이 워크트리끼리 공유된다. 변경을 잠시 치울 때는 임시 커밋이나 `target/` 아래의 사본을 쓴다.
- **닫힌 로컬 포트로의 연결은 거절되지 않고 매달릴 수 있다**(WSL2). 포트 탐침에는 제한 시간을 둔다(xtask의 탐침은 2초다).

## 4. 웹앱과 맞닿는 것

- 계약 파일 `contracts/openapi.json`. 웹앱의 타입 `apps/web/src/lib/api/contract.ts`는 `openapi-typescript`로 이 파일에서 만든다.
- 날짜 값의 시각 표기는 UTC 밀리초 정수다.
- WebSocket의 메시지는 OpenAPI 밖의 `contracts/websocket.md`와 `websocket.json`에 있다. 카드 본문을 함께 고치는 방(명령, 메시지, Binary 프레임, 세대, 한도)은 그 6절이다.
- 바꾸는 요청은 `fetch`의 기본 방식(`cors`)으로 보낸다. 서버가 페이지에 `Referrer-Policy: no-referrer`를 붙인다. 그래서 다른 방식(`same-origin`, `no-cors`, 브라우저가 직접 보내는 폼)의 요청은 `Origin: null`이 되어 403이다(계약의 머리말).
- API 밖의 주소는 `/ws`, `/static/…`, 웹앱의 화면뿐이다. `ready`와 `ping`도 `/api/v1` 아래에 있다.
