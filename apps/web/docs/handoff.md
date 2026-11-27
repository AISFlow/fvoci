# 웹앱 개발 안내

경로는 따로 말하지 않으면 `apps/web` 기준이다.

다른 문서에 없는 것만 적는다. 이미 적힌 것은 위치만 가리킨다.

## 지금 상태

- **화면은 서버에 붙어 있다.** 서버가 서빙하는 앱은 서버를 부르는 구현(`src/lib/api/`)으로 돌고, 서버 없이 빌드를 보는 도구(`preview`, `look`)와 개발 서버는 메모리 구현으로 돈다. 서버에 붙지 않은 조작은 없다. 비밀번호를 받지 않는 조직의 401 `sso_required`는 로그아웃이 아니다(`state/problems.ts`의 `isSignedOut`). 공급자가 바뀐 뒤 그 전에 공급자로 만든 세션이 토큰을 만들면 409 `provider_changed`다: 세션은 그대로이고 401로 답하지 않는다.
- **계약**은 저장소 루트의 `contracts/`다: `openapi.json`(`src/lib/api/contract.ts`를 `pnpm --filter web run api:types`로 여기서 만든다), `websocket.md`, 본문·Y 문서·커서·이름의 벡터(`*-vectors.json`).
- **서버와 한 브랜치에 있다.** 브랜치 `main`에 서버(저장소 루트)와 웹앱(`apps/web`)이 함께 있다. 양쪽에 걸친 안내(브랜치, 푸시, 검증)는 저장소 루트의 `docs/handoff.md` 맨 앞에 있다.

## 어디에 무엇이 있는가

| 알고 싶은 것                                  | 있는 곳                                                                                |
| --------------------------------------------- | -------------------------------------------------------------------------------------- |
| 구조와 규칙, 고정한 판, 글꼴, 순수 로직, 미결 | `docs/notes.md`                                                                        |
| 화면마다의 코드가 있는 곳                     | `docs/notes.md`의 "화면과 데이터의 경계"                                               |
| 날짜 값의 와이어, 지역 설정의 값              | `docs/notes.md`의 "날짜 속성 값의 와이어 모양", "보는 사람의 시간대: 계정 설정"        |
| 내장 템플릿                                   | 저장소 루트의 `crates/server/assets/templates/`의 `README.md`                          |
| 속성 편집기                                   | `src/lib/components/README.md`                                                         |
| 빌드의 `.zst`                                 | `docs/build-notes.md`                                                                  |
| 화면의 그림                                   | 커밋하지 않는다. `FVOCI_SCREENS=1`로 찍으면 `target/screens/<화면>/`(`e2e/screens.ts`) |
| 도구, 화면을 띄우는 법, 함정                  | 이 문서                                                                                |

## 검증

명령은 `docs/notes.md`의 "검증 명령"에 있다. `stories/`·`.storybook/`·공용 컴포넌트를 바꾼 묶음은 `pnpm --filter web run test:storybook`을 따로 돌린다(`check`에는 포함하지 않는다. PR에서는 CI가 돈다).

## 도구

- Node는 시스템의 것을 쓴다(v26).
- `pnpm`은 루트 `package.json`의 `packageManager`가 정한 판(12.21.0)이다. 전역에 없으면 저장소 루트에서 한 번 설치하고, 명령마다 `PATH`를 앞에 붙인다.

  ```bash
  cd <워크트리 루트>
  npm install --prefix target/tools/node pnpm@12.21.0
  export PATH=$PWD/target/tools/node/node_modules/.bin:$PATH
  ```

- 종단 테스트의 브라우저는 Playwright의 Chromium headless shell이다. `e2e:apple`·`e2e:live:apple`은 WebKit(데스크톱 Safari, iPhone, iPad의 크기)을 쓴다.
- 셸은 명령 사이에 `PATH`와 작업 폴더가 남지 않는다고 보고, 명령마다 다시 적는다.

## 화면을 띄우는 법과 포트

| 무엇                          | 명령                              | 포트                          | 주소               |
| ----------------------------- | --------------------------------- | ----------------------------- | ------------------ |
| **앱과 Storybook을 한꺼번에** | `pnpm --filter web run look`      | 18410, 모든 인터페이스        | `/`가 목록이다     |
| 앱 개발 서버                  | `pnpm --filter web run dev`       | 5173                          | `/`                |
| 빌드 결과 미리 보기           | `pnpm --filter web run preview`   | 4173                          | `/`                |
| 앱 Playwright                 | `e2e`                             | 18310, `FVOCI_E2E_PORT`       | 스스로 띄우고 끈다 |
| 실제 서버의 Playwright        | `e2e:live`(아래)                  | 18510, `FVOCI_LIVE_PORT`      | 스스로 띄우고 끈다 |
| Storybook Playwright          | `test:storybook`                  | 18369, `FVOCI_STORYBOOK_PORT` | 스스로 띄우고 끈다 |
| Storybook 개발 서버           | `pnpm --filter web run storybook` | 6006                          | `/`                |

여러 묶음을 나란히 돌리면 `FVOCI_E2E_PORT`·`FVOCI_LIVE_PORT`·`FVOCI_STORYBOOK_PORT`를 묶음마다 다르게 정한다.

`e2e:live`에 관해:

- 아카이브 검사에 `unzip`을 쓴다.
- 먼저 저장소 루트에서 `cargo build -p fvoci-server --bins --examples`와 `pnpm --filter web run build`를 한다. 지어 둔 `dist/`를 서빙하므로, 웹을 바꾼 뒤에는 `build`부터 다시 한다.
- `FVOCI_STORE_TEST_CONN_STRING`에 CREATE 권한이 있는 NOSUPERUSER·NOBYPASSRLS PostgreSQL URL을 주고 `pnpm --filter web run e2e:live`를 실행한다.
- `target/web-live/`에 띄우는 것: SQLite 서버와 다음 포트의 가짜 OIDC 공급자(조직이 옮겨 가는 두 번째 공급자는 다섯 칸 뒤), 두·세 칸 뒤 포트의 가짜 SMTP와 그 받은편지함, 네 칸 뒤 포트의 열린 가입 PostgreSQL 서버, 여덟 칸 뒤 포트의 가입 주소를 메일로 확인하는 SQLite 서버(`email_verification`; 두 서버의 가짜 SMTP는 저마다 두·세 칸 뒤), 아홉 칸 뒤 포트의 가짜 검색 서버(`tools/live-server.ts`). 그래서 묶음마다의 `FVOCI_LIVE_PORT`는 서로 열두 칸 이상 띄운다.
- 일꾼은 하나다(`playwright.live.config.ts`의 `workers: 1`). 테스트들이 서버 하나의 SSO 상태를 함께 쓴다.
- 끝나면 서버와 전용 PostgreSQL 스키마를 정리한다. 테스트는 `e2e-live/`에 있다.

`look`에 관해:

- `build`, `build-storybook`을 한 뒤 `tools/serve.ts`로 `dist/`, `storybook-static/`을 서빙한다. 정적 파일뿐이고 API는 없다. 목록의 주소는 `/app/`(조립된 앱)과 `/storybook/`이다.
- 앱은 서버가 하듯 base path `/app` 아래에서, 콘텐츠 보안 정책을 붙여서 나간다.
- `/app/?dev=1`로 열면 화면 구석에 창의 너비와 지금의 단계가 보인다.
- 앱의 데이터는 메모리 구현의 것이라 새로 고치면 처음으로 돌아간다. 표시 설정만 기기에 남는다.
- 요청마다 파일을 읽고, 캐시하지 않게 보낸다. **서버를 띄워 둔 채 빌드만 다시 하면 새 파일이 나간다.**
- Storybook도 앱의 글꼴을 싣는다.
- WSL2에서 띄우면 Windows의 브라우저에서는 `http://localhost:18410/`로 연다. 기기의 LAN 주소로는 WSL의 방화벽 설정에 따라 닿지 않을 수 있다.

## 확인하지 못한 것

- 종단 테스트는 Chromium의 세 크기(1280×720, 626×890 터치, Pixel 7)와 WebKit의 세 기기다. 실제 기기, Firefox, 화면 낭독기, 한글 조합 입력, 터치 드래그, 진동, 접기·회전, 안전 영역은 테스트가 보지 않는다.
- 이모지 글꼴이 없는 기기에서 찍은 화면의 그림에서는 이모지가 빈 네모로 나온다.
- "움직임 줄이기"에서는 전환이 0.01ms로 줄어든다. 그 상태에서 "놓자마자 재는" 코드는 잘못 잴 수 있다(다이어그램에서 한 번 그랬다).

## 걸렸던 함정

### 기기

- **닫힌 로컬 포트로의 연결이 거절되지 않고 매달릴 수 있다**(WSL2). `curl`은 `-m`으로 끊는다. Playwright는 `webServer.url`이나 `port`가 있으면 서버를 띄우기 전에 그 주소가 비었는지 연결해 보는데, 거기서 멈출 수 있다. 그래서 `url` 대신 `wait: { stdout: … }`로 서버의 준비 출력을 기다린다.

### 빌드와 서빙

- **Storybook 빌드에는 앱의 산출물 플러그인을 빼야 한다.** `.storybook/main.ts`의 `viteFinal`에서 `fvoci:` 플러그인, `renderBuiltUrl`, 해시 파일명 설정을 걷어 낸다.
- **글꼴 산출물은 커밋되지 않는다.** 새 워크트리는 `src/styles/fonts/`가 비어 있고, `build`가 다시 만들려면 글꼴 원본(약 143 MB)을 받는다. 한 번 만든 `src/styles/fonts/`와 `target/web-fonts/`를 다른 워크트리에 복사해 두면 `build`가 네트워크 없이 지나간다.
- **글꼴 도구의 건너뛰기 판단은 `tools/fonts/` 아래 모든 파일의 해시다.** 주석 하나만 고쳐도 다음 `build`가 글꼴을 다시 만든다(약 2분). 복사해 둔 것도 낡는다.
- **글꼴은 늦게 도착한다.** 규칙은 브라우저가 한가할 때, 파일은 글자가 처음 필요할 때 온다. 도착하면 줄이 바뀐다. 두 쪽을 차례로 재서 견주는 브라우저 테스트는 그 사이에 걸리므로, `networkidle`과 `document.fonts.ready`를 기다린 뒤 재는 동안 아무것도 움직이지 않았는지까지 본다.

### 로직과 타입

- **`PropertyValue`는 넓다**(날짜 값이 객체다). 글자나 목록을 기대하는 자리는 `plainPropertyValue`를 거친다. `String(값)`을 쓰면 린트(`no-base-to-string`)가 잡는다.
- **`Date.UTC`는 0–99년을 1900–1999년으로 읽는다.** 서버가 받는 연도는 1–9999라서 `dates.ts`가 `setUTCFullYear`로 만든다.
- **`formatDateValue`는 빈 값에 `undefined`를 준다.** 테스트에서는 `tests/logic/support/factory.ts`의 `storedDate`를 쓴다.
- **Tiptap(ProseMirror)의 `marks: ''`는 "이 노드 안에 올 수 있는 마크"다.** 노드 자신이 마크를 갖는지는 부모가 정한다. 그래서 줄바꿈에 마크가 붙지 않게 하는 데는 쓸 수 없고, 저장할 때 `toStoredDoc`이 글자가 아닌 노드의 마크를 뗀다.
- **검증기(`validateDoc`)는 어떤 값에도 예외를 던지지 않아야 한다.** 새 규칙을 더할 때 `tests/editor/schema.test.ts`의 "a value that is not shaped like a document"에 경우를 더한다.

### 생성물과 git

- **생성물을 Prettier가 다시 접는다.** `JSON.stringify(…, null, 2)`의 배열 모양은 Prettier의 것과 다르다. 도구가 쓰는 파일은 `.prettierignore`에 넣고, "최신인가"는 검사가 본다(`src/lib/api/contract.ts`).
- **충돌을 풀며 체리픽한 커밋은 `git cherry`가 "같은 변경"으로 알아보지 못한다.** 워크트리를 지워도 되는지는 워킹 트리가 깨끗한지와 내용이 들어왔는지를 따로 확인한다.
- **다른 커밋의 트리를 `target/` 아래에 풀고 거기서 `pnpm exec`를 부르면 안 된다.** pnpm이 심볼릭 링크를 타고 `apps/web/node_modules`에 잠금 파일 없이 설치한다. 그렇게 됐으면 `node_modules`를 지우고 `pnpm install --frozen-lockfile`로 다시 설치한다.

## 추적되지 않는 것

- 무시되는 산출물은 `dist/`, `storybook-static/`, `test-results/`, `src/styles/fonts/`, `src/lib/i18n/paraglide/`, `.wrangler/`, `tools/storybook/wrangler/node_modules/`다. 모두 다시 만들 수 있다.
- 가공하기 전의 글꼴과 중간 산출물은 `target/web-fonts/`에, 실서버 검증의 서버와 로그는 `target/web-live/`에 생긴다.
