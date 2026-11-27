# fvoci 웹앱 노트

`apps/web`의 구조와 규칙, 고정한 판, 글꼴, 순수 로직의 규칙을 적는다. 경로는 `apps/web` 기준이고 시각은 모두 KST다.

- 도구, 화면을 띄우는 법과 포트, 걸렸던 함정은 `docs/handoff.md`에 있다.
- W0–W6은 계획의 단계 이름이다: W0 준비, W1 순수 로직, W2 뼈대와 얇은 종단 조각, W3 보드 페이지, W4 카드, W5 나머지 뷰와 기능, W6 모바일 마무리. S6은 같은 계획의 서버 쪽 단계다.
- Q1–Q16은 "정렬, 필터, 검색, 계산의 규칙"의 번호다.
- 글꼴과 압축을 말하는 곳의 "원본"은 가공하기 전의 파일을 뜻한다.

## 검증 명령

워크스페이스 루트에서 실행한다. 패키지 이름이 `web`이다. `pnpm`을 놓는 법은 `docs/handoff.md`의 "도구"에 있다.

```bash
pnpm install --frozen-lockfile
pnpm --filter web run check     # paraglide 컴파일 → svelte-check → eslint → prettier --check → 라이선스 검사 → 이름 검사 → 로케일 키 집합 검사
pnpm --filter web run test      # paraglide 컴파일 → vitest를 TZ=UTC, Asia/Seoul, Pacific/Kiritimati 로 세 번
pnpm --filter web run build     # paraglide 컴파일 → 글꼴 도구 → vite build
pnpm --filter web run e2e       # Playwright. 먼저 build 가 있어야 한다
```

- `check`에 `tsc`를 따로 넣지 않았다. `svelte-check`가 `tsconfig.json`의 `include` 전체(`.ts` 포함)를 같은 설정으로 검사한다. `tools/*.ts`와 `vite.config.ts`의 타입 오류가 `svelte-check` 출력에 나오는 것을 확인했다.
- `test`는 단위 테스트를 세 시간대로 돌린다(`tools/test-zones.ts`). 하나라도 실패하면 실패다. 한 번만 돌리려면 `test:once`.
- `e2e`는 `FVOCI_E2E_URL`이 있으면 그 서버를, 없으면 `tools/preview.ts`를 띄워 대상으로 삼는다. `preview.ts`에는 API가 없다. 제품 시나리오는 브라우저 안의 메모리 구현 위에서 돈다.
- `e2e`의 프로젝트는 셋이다: `desktop`, `mobile`, `middle`(626×890, 터치). 글꼴 측정 7건은 `desktop`에서만 돈다.
- 번역: `i18n:keys`(키 집합 검사).
- 보조 스크립트: `i18n`(메시지 컴파일), `fonts`(글꼴 도구), `licenses`, `lint`, `format`, `preview`(빌드 결과를 서버 규칙대로 서빙), `storybook`, `build-storybook`, `look`(앱과 Storybook을 한 포트에서 본다. `docs/handoff.md`).

## 프로젝트 구조

```
apps/web/
  index.html               서버가 {{.BaseURL}} 을 채우는 뼈대
  src/app/                 진입점, 주소, 보드 페이지, 어느 구현을 쓸지 고르는 자리
  src/lib/logic/           순수 로직
  src/lib/state/           데이터의 경계(Backend), 메모리 구현, 저장소
  src/lib/layout/          화면 크기의 세 단계, 안전 영역, 뒤로 가기
  src/lib/drag/            드래그의 기반
  src/lib/editor/          본문 문서: 허용 목록, 표시기, 편집기
  src/lib/ui/              shadcn-svelte 생성물 (README 에 수정 기록)
  src/lib/components/      직접 쓴 공용 부품
  src/lib/features/        화면: shell, view-header, views, card
  src/lib/i18n/paraglide/  메시지 컴파일 결과 (생성물, git 무시)
  src/lib/fonts.ts         @font-face 규칙의 지연 로드
  src/lib/theme.svelte.ts  밝은·어두운 화면 상태
  src/styles/app.css       디자인 토큰
  src/styles/fonts/        글꼴 도구의 산출물 (git 무시)
  messages/                메시지 18 로케일 (손으로 고치는 원천)
  tests/                   vitest: logic, state, editor, tokens, tools (node), ui (jsdom)
  e2e/                     Playwright (제품)
  stories/                 Storybook
  tools/                   build, emoji, fonts, i18n, licenses, lint, math, names, preview, serve
  docs/                    문서
```

루트에 추가한 파일은 셋이다: `package.json`(`packageManager` 고정), `pnpm-workspace.yaml`, `pnpm-lock.yaml`. 워크스페이스 구성원은 `apps/*`뿐이다. `apps/web/.gitignore`를 따로 뒀다.

## 구조 규칙

**지금의 규칙은 `src/lib/README.md`의 표다.** `src/lib` 바로 아래의 폴더가 층이고(0 `logic`·`i18n`·`hooks`·`utils`·`theme`, 1 `regional`·`display`·`fonts`·`layout`·`drag`·`editor`, 2 `state`·`ui`, 3 `components`, 4 `features`), 파일은 자기 층과 그보다 낮은 층에서만 가져온다(`fvoci/layer-order`). 공용 부품(`components`)은 `state`를 가져오지 못하고, 화면은 서버를 직접 부르지 못한다(`fvoci/no-direct-server`). 속성 편집기는 `features/properties`에서 `components/properties`로 내렸다. 아래 문단은 처음 세운 규칙의 기록이고 지금도 그대로 검사된다.

`src/lib/features/<기능>/` 폴더끼리는 서로 가져오지 못한다. 공유할 것은 `lib/logic`, `lib/state`, `lib/api`, `lib/ui`로 내린다. 반대로 그 아래 층은 기능 폴더에서 가져오지 못한다. ESLint 규칙 `fvoci/feature-boundaries`(`tools/lint/layers.ts`, 직접 쓴 규칙이라 의존성이 늘지 않았다)가 검사하고 `check`에 들어 있다. `$lib/features/x`와 `../x` 두 가지 쓰기를 모두 잡는다. 규칙 자체의 테스트는 `tests/tools/layers.test.ts`(5건). 설명은 `src/lib/features/README.md`.

## 고정한 판

2026-11-10 16:50 KST에 확인했고, 2026-11-16 10:44–11:47 KST에 다시 올렸다(Tiptap과 `tailwind-variants` 4까지).

출처는 npm 레지스트리다. 패키지마다 `npm view <이름> version`(dist-tag `latest`)과 `npm view <이름> "time[<판>]"`(공개 시각)을 조회했고, 호환 범위는 `npm view <이름> peerDependencies`로 확인했다. 기억으로 정한 판은 없다.

기준: `latest`를 쓴다. 다만 공개된 지 24시간이 안 된 판은 직전 판으로 내린다. pnpm의 `minimumReleaseAge` 기본값이 1440분(1일)이고(출처: pnpm 문서 settings/dependency-resolution), 이것을 `pnpm-workspace.yaml`에 명시해 조건에 안 맞으면 설치가 실패하게 했다.

| 패키지                                               | 고정 판 | 공개 (UTC) | 메모                                                                                                                                                                                                     |
| ---------------------------------------------------- | ------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| pnpm                                                 | 12.21.0 | 2026-11-13 | `packageManager`에 sha512와 함께 적었다                                                                                                                                                                  |
| svelte                                               | 5.57.3  | 2026-10-26 |                                                                                                                                                                                                          |
| vite                                                 | 8.3.8   | 2026-11-11 |                                                                                                                                                                                                          |
| @sveltejs/vite-plugin-svelte                         | 7.3.1   | 2026-09-23 |                                                                                                                                                                                                          |
| typescript                                           | 6.0.3   | 2026-04-16 | `latest`는 7.0.2. 7의 패키지에는 컴파일러 API가 없고, `svelte-check` 4.7.8(peer `^5 \|\| ^6`), `typescript-eslint` 8.72.2(`<6.1.0`), `openapi-typescript` 7.13.0(`^5.x`)이 받지 않아 6.x의 최신으로 뒀다 |
| tailwindcss, @tailwindcss/vite                       | 4.3.5   | 2026-11-06 |                                                                                                                                                                                                          |
| bits-ui                                              | 2.21.0  | 2026-11-11 |                                                                                                                                                                                                          |
| shadcn-svelte (CLI, CSS)                             | 1.8.1   | 2026-11-11 | `shadcn-svelte/tailwind.css`를 가져오므로 의존성이기도 하다                                                                                                                                              |
| cn                                                   | 0.5.2   | 2026-11-02 | 생성된 `utils.ts`가 쓴다                                                                                                                                                                                 |
| tailwind-variants                                    | 4.0.2   | 2026-11-10 | 4.0에서 `tailwind-merge` 대신 미리 엮은 병합 표를 쓴다. `src/lib/ui`의 `tv()`가 내는 클래스는 3.3.1과 같다                                                                                               |
| tw-animate-css                                       | 1.4.0   | 2025-09-24 |                                                                                                                                                                                                          |
| @lucide/svelte                                       | 1.73.0  | 2026-11-14 |                                                                                                                                                                                                          |
| svelte-sonner                                        | 1.2.3   | 2026-11-02 |                                                                                                                                                                                                          |
| @internationalized/date                              | 3.12.4  | 2026-09-01 | bits-ui의 peer, 달력 컴포넌트가 쓴다                                                                                                                                                                     |
| @inlang/paraglide-js                                 | 3.2.2   | 2026-11-10 |                                                                                                                                                                                                          |
| @inlang/plugin-message-format                        | 4.6.0   | 2026-11-03 | `project.inlang/settings.json`이 CDN 주소가 아니라 `node_modules`의 파일을 가리킨다                                                                                                                      |
| vitest                                               | 5.1.0   | 2026-11-02 |                                                                                                                                                                                                          |
| jsdom                                                | 31.0.1  | 2026-11-13 |                                                                                                                                                                                                          |
| @testing-library/svelte                              | 5.4.2   | 2026-06-23 |                                                                                                                                                                                                          |
| @testing-library/jest-dom                            | 7.0.1   | 2026-08-09 |                                                                                                                                                                                                          |
| @playwright/test                                     | 1.65.1  | 2026-11-13 | Chromium 빌드 1255                                                                                                                                                                                       |
| storybook, @storybook/svelte, @storybook/svelte-vite | 10.6.7  | 2026-11-10 |                                                                                                                                                                                                          |
| svelte-check                                         | 4.7.8   | 2026-11-01 |                                                                                                                                                                                                          |
| eslint                                               | 10.15.0 | 2026-11-13 |                                                                                                                                                                                                          |
| @eslint/js                                           | 10.0.1  | 2026-02-06 |                                                                                                                                                                                                          |
| typescript-eslint                                    | 8.72.2  | 2026-11-09 |                                                                                                                                                                                                          |
| eslint-plugin-svelte                                 | 3.24.1  | 2026-11-07 |                                                                                                                                                                                                          |
| eslint-config-prettier                               | 10.1.8  | 2025-07-18 |                                                                                                                                                                                                          |
| globals                                              | 17.14.0 | 2026-11-01 |                                                                                                                                                                                                          |
| prettier                                             | 3.10.1  | 2026-11-12 |                                                                                                                                                                                                          |
| prettier-plugin-svelte                               | 4.1.1   | 2026-06-15 |                                                                                                                                                                                                          |
| prettier-plugin-tailwindcss                          | 0.8.1   | 2026-07-15 |                                                                                                                                                                                                          |
| @types/node                                          | 26.6.6  | 2026-10-28 | Node 26.13.0에서 작업했다                                                                                                                                                                                |
| subset-font                                          | 2.9.0   | 2026-09-20 | 글꼴 도구. HarfBuzz의 WebAssembly 빌드를 쓴다                                                                                                                                                            |
| @tiptap/core, @tiptap/pm, 확장 15개                  | 3.34.0  | 2026-11-13 | 편집기. 묶음(`starter-kit`)은 쓰지 않는다                                                                                                                                                                |
| marked                                               | 18.2.0  | 2026-11-09 | 마크다운 가져오기                                                                                                                                                                                        |
| fast-check                                           | 4.10.2  | 2026-09-19 | 속성 기반 테스트(2026-11-10 18:45 KST 확인)                                                                                                                                                              |

글꼴 원천 파일과 유니코드 자료(2026-11-10 17:07 KST 확인, `tools/fonts/sources.json`에 주소와 SHA-256):

| 자료                                          | 판                                                         | 출처                                           |
| --------------------------------------------- | ---------------------------------------------------------- | ---------------------------------------------- |
| Noto Sans, Noto Sans KR·JP·SC·TC (가변, wght) | `google/fonts` 커밋 `5bb28733` (2026-11-08)                | GitHub API로 커밋과 파일 크기 확인             |
| Noto Serif, Noto Serif KR·JP·SC·TC            | 같은 커밋                                                  | 도구가 지원만 한다                             |
| D2Coding Regular·Bold                         | 1.4.0 (`naver/d2-coding-font` 태그 `VER1.4.0`, 2026-10-03) | GitHub 릴리스 목록. 1.4.0에도 가변 글꼴은 없다 |
| Unihan                                        | 유니코드 18.0.0                                            | `unicode.org/Public/UCD/latest`의 ReadMe       |

## 빌드 산출물

`dist/index.html` + `dist/static/`이다. 서버의 `web_app_path`를 `dist`로 두면 바로 서빙된다.

- **파일명의 해시는 16자리 16진수다.** 서버(`crates/server/src/web.rs`의 `has_content_hash`)는 이름을 `-`, `.`, `_`로 나눈 조각 가운데 "8자 이상의 16진수이고 숫자가 하나 이상"인 것이 있어야 내용 해시로 보고 `immutable`을 붙인다. Vite의 기본 해시는 base64 계열이라 이 조건에 안 맞는다. `hashCharacters: 'hex'`로 바꾸고, `static/` 아래에 조건을 못 맞춘 파일이 있으면 빌드가 실패하게 했다(`tools/build/output.ts`).
- **base path를 빌드에 넣지 않는다.** `index.html`은 `{{.BaseURL}}/static/…`으로, 스크립트와 스타일시트끼리는 상대 경로로 참조한다(`experimental.renderBuiltUrl`). `/boards` 아래의 깊은 주소에서 파일이 모두 풀리는 것을 확인했다.
- **사전 압축.** `static/` 아래의 텍스트 파일마다 `.br`(brotli 11), `.zst`(zstd, 2026-11-10에 더함. `docs/build-notes.md`)와 `.gz`(gzip 9)를 만든다. woff2는 이미 압축돼 있어 뺀다. `index.html`은 서버가 요청마다 치환해 보내므로 압축하지 않는다.
- **고지.** `static/THIRD_PARTY_NOTICES.txt`. 이름에 해시가 없어 서버가 `no-cache`로 보낸다(의도).

서버 쪽에서 빌드한 실제 Rust 서버에 `web_app_path`를 이 `dist`로 주고 확인한 것(2026-11-10 17:24 KST, 서버는 읽기만 했다): `{{.BaseURL}}` 치환, 해시 파일의 `Cache-Control: public, max-age=31536000, immutable`, `Accept-Encoding`에 따른 `br`·`gzip`·무압축 응답과 `Vary`, 고지 파일의 `no-cache`, woff2의 `font/woff2`. `FVOCI_E2E_URL`로 그 서버를 대상으로 한 Playwright 6건도 통과했다.

글꼴 규칙 CSS의 크기(2026-11-10 17:33 KST, 바이트. 한국어의 것은 그 뒤 파일이 셋이 되어 압축 전 151,944바이트가 됐다):

| 파일                       | 원본    | br     | gzip   |
| -------------------------- | ------- | ------ | ------ |
| `faces-noto-sans-kr-*.css` | 87,765  | 9,306  | 35,646 |
| `faces-noto-sans-jp-*.css` | 131,281 | 13,860 | 48,756 |
| `faces-noto-sans-sc-*.css` | 103,587 | 11,195 | 22,367 |
| `faces-noto-sans-tc-*.css` | 187,395 | 19,362 | 63,474 |
| `faces-d2coding-*.css`     | 59,847  | 6,369  | 12,244 |

글꼴 규칙 CSS는 같은 `unicode-range` 목록이 굵기 구간마다 되풀이되어 brotli에서 크게 줄고 gzip에서는 덜 줄어든다(gzip의 창이 32KB라 되풀이를 못 본다).

주 스타일시트에는 Tailwind가 `src/lib/ui`의 컴포넌트가 쓰는 클래스를 모두 낸다. 화면이 쓰는 것만 남기는 것이 아니라 소스에 나오는 것을 낸다. 스크립트와 스타일의 지금 크기는 아래 "번들 크기"에 있다.

### 번들 크기 (조립된 앱, 2026-11-11 01:36 KST)

브라우저가 실제로 받는 파일을 단계마다 세었다. 조건은 1280×720, 영어 화면, 첫 보드는 칸반이다. 측정 스크립트 `target/scratch/measure-sizes.mjs`가 띄운 서버에서 요청을 모으고 `dist`의 파일 크기를 더한다. 그 스크립트는 저장소에 없다. 글꼴은 이미 압축돼 있어 그대로 센다. 단위는 바이트다.

| 단계                                               | 파일 | 원본      | gzip      | brotli    | zstd      |
| -------------------------------------------------- | ---- | --------- | --------- | --------- | --------- |
| **첫 화면**(보드, 칸반)                            | 25   | 5,224,878 | 3,833,326 | 3,634,722 | 3,663,122 |
| 그 가운데 스크립트                                 | 16   | 1,143,815 | 308,933   | 243,828   | 262,828   |
| 그 가운데 스타일                                   | 6    | 775,679   | 219,009   | 85,510    | 94,910    |
| 그 가운데 글꼴                                     | 3    | 3,305,384 | —         | —         | —         |
| **카드를 열 때** 더 받는 것(`BodyEditor-*.js`)     | 1    | 413,119   | 130,640   | 110,727   | 116,364   |
| **수식**이 본문에 있을 때 더 받는 것               | 6    | 354,470   | 152,339   | 138,484   | 143,980   |
| 그 가운데 `katex-*.js`                             | 1    | 259,382   | 76,680    | 63,522    | —         |
| 그 가운데 `katex-*.css`                            | 1    | 22,497    | 3,489     | 2,882     | —         |
| 그 가운데 글꼴(수식 둘, 고정폭 하나)               | 3    | 71,424    | —         | —         | —         |
| **다이어그램**(순서도)이 본문에 있을 때 더 받는 것 | 27   | 827,581   | 222,834   | 192,683   | 207,142   |
| 그 가운데 가장 큰 조각                             | 1    | 241,442   | 37,250    | 30,085    | —         |

- 첫 화면의 스크립트는 거의 `index-*.js` 하나다(985,075 / gzip 273,838 / brotli 213,795). 화면 여섯과 공용 부품, 저장소, 메시지가 한 덩어리로 들어 있다.
- 나머지 15개(합쳐 158,740바이트)는 메모리 구현의 조각 둘(구현, 씨앗)과 그것이 읽는 내장 템플릿 13개다. **이 15개는 서버를 부르는 구현이 들어오면 받지 않게 된다.**
- 화면별로 쪼개는 일과 상한을 정하는 일은 하지 않았다.
- 첫 화면의 글꼴 3.3 MB 가운데 2.67 MB는 `noto-sans-kr-rest`다. 본보기 구성원의 이름에 든 한자(陳美玲)가 "자주 쓰는 글자" 묶음 밖이라 나머지 묶음을 통째로 부른다. 영어 화면에 한글·한자 이름이 없으면 `noto-sans-common` 112,876바이트만 받는다.
- 편집기(`BodyEditor`)는 카드를 처음 열 때 받는다. 수식과 다이어그램은 본문에 그것이 있을 때만 받는다(읽기 전용 표시기도 같은 조각을 쓴다). 다이어그램은 종류마다 조각이 따로라 순서도가 아니면 수치가 다르다.

## 디자인 토큰 (`src/styles/app.css`)

- **색.** 밝은 화면 `:root`, 어두운 화면 `.dark`. 출발점은 생성기의 `neutral` 팔레트이고 다음을 바꿨다.
  - `--input-border`(새 토큰): 입력 칸의 테두리. 생성기의 `--input`은 바탕과 1.2:1이라 테두리로 쓸 수 없다.
  - `--ring`, `--sidebar-ring`: 초점 표시. 회색(2.6:1)에서 파란색으로(밝은 화면 5.0:1, 어두운 화면 7.4:1).
  - `--muted-foreground`: 밝은 화면에서 `--muted` 위 4.3:1이던 것을 5.1:1로.
  - `--destructive`: 삭제 버튼은 같은 색의 옅은 바탕 위에 같은 색 글자다(`bg-destructive/10`, 호버 `/20`). 호버 상태에서 3.8:1이던 것을 4.6:1로(대비 테스트가 잡았다).
  - 브랜드 색은 정해진 것이 없어 무채색 그대로다.
- **글자 단계.** 모두 rem이다. `--text-sm`이 UI 기본이고 0.9375rem(15px)이다. `--text-base`는 1rem으로, 본문과 좁은 화면의 입력 칸에 쓴다. 뷰포트 메타에서 `maximum-scale`과 `user-scalable`을 뺐다.
- **줄 높이.** UI 1.45, 본문 1.6. `:lang(ko)`, `:lang(ja)`, `:lang(zh)`에서는 1.55, 1.75. `lang`이 다른 하위 트리에도 적용되도록 `@theme inline`으로 변수 자체를 참조하게 했다.
- **글꼴 스택.** 언어마다 스택 전체를 정한다(`--font-sans-stack`). 한국어는 `'Noto Sans KR'`, 일본어는 `'Noto Sans JP'`가 맨 앞이고 그다음이 `'Noto Sans'`, 나머지 CJK family다. 중국어 두 로케일과 그 밖의 언어는 `'Noto Sans'`가 맨 앞이고 CJK 네 family가 현재 언어의 것부터 뒤따른다. 끝은 시스템 글꼴. 고정폭은 `'D2Coding'`. (2026-11-10 20:02 KST에 바꿈. 아래 "글꼴")
- **간격·모서리.** `--spacing` 0.25rem, `--spacing-touch` 2.75rem(손가락으로 누르는 대상의 최소 변), `--radius` 0.5rem.
- **초점.** 자체 표시가 없는 요소는 `--ring` 색의 2px 윤곽선. 생성 컴포넌트는 같은 색의 테두리와 링을 쓴다.
- **대비 검사.** `tests/tokens/contrast.test.ts`가 `app.css`를 직접 읽어 두 화면 각각 글자 15쌍(4.5:1), 입력 테두리·초점 11쌍(3:1), 삭제 버튼의 옅은 바탕을 검사한다. 반투명 색은 브라우저처럼 감마 인코딩된 sRGB에서 합성해 계산한다.

## 글꼴 (`tools/fonts`)

`pnpm --filter web run fonts`(= `build`의 한 단계)가 원본을 내려받아(SHA-256 검사, `target/web-fonts/src`에 캐시) family마다 "상용" 파일 하나와 "잔여" 파일 하나를 WOFF2로 만들고 `@font-face` 규칙을 쓴다. **한국어만 파일이 셋이다**(상용, 나머지 음절, 잔여). 산출물(`src/styles/fonts/`)은 커밋하지 않는다. 입력이 같으면 건너뛴다. 전체 빌드는 이 기기에서 1분 40초이고, 건너뛸 때의 `build`는 2초다.

사용자 결정(2026-11-10)으로 바꾼 것 둘(2026-11-10 20:02 KST 적용):

1. **한국어는 세 파일**: 상용 파일에는 KS X 1001의 한글 2,350자만 넣고, 나머지 현대 음절 8,822자는 따로 `noto-sans-kr-syllables.woff2`로 뗀다. 드문 음절(`똠`, `쌰`, `뷁`)이 나오면 이 0.78 MB만 받고 잔여(2.67 MB)는 받지 않는다. 일본어·중국어의 글자 집합은 그대로다.
2. **한국어·일본어 로케일은 자기 CJK 글꼴이 스택 맨 앞이다.** 영문·숫자·문장부호도 그 글꼴의 것으로 그린다(말줄임표가 가운데 높이에 찍힌다). 그래서 KR·JP의 상용 파일에 기본 라틴, 라틴-1, 일반 문장부호를 넣었다. 중국어 두 로케일은 전처럼 라틴 글꼴이 앞이다(중국어 글꼴의 전각 따옴표가 영문 `it’s`를 벌린다).

글자 집합 정의(커밋됨):

| family       | 상용 파일에 들어가는 것                                                                                            | 정의                                                                       |
| ------------ | ------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| Noto Sans    | 기본 라틴, 라틴-1, 라틴 확장-A, 결합 부호, 일반 문장부호, 통화·글자꼴 기호 등                                      | `charsets.ts`의 블록                                                       |
| Noto Sans KR | KS X 1001의 한글 2,350자, 호환 자모, CJK 문장부호, 전각 기호, KS X 1001의 기호, **기본 라틴·라틴-1·일반 문장부호** | `charsets/ks-x-1001-hangul.txt` (2,350자), `ks-x-1001-symbols.txt` (986자) |
| Noto Sans JP | 히라가나, 가타카나, 상용한자 2,136자, JIS X 0208의 기호, **기본 라틴·라틴-1·일반 문장부호**                        | `charsets/joyo.txt`, `jis-x-0208-symbols.txt` (524자)                      |
| Noto Sans SC | 통용규범한자표 1급 3,500자, GB 2312의 기호                                                                         | `charsets/tgh-level1.txt`, `gb-2312-symbols.txt` (728자)                   |
| Noto Sans TC | Big5 1수준 5,401자, 주음부호, Big5의 기호                                                                          | `charsets/big5-level1.txt`, `big5-symbols.txt` (409자)                     |
| D2Coding     | 기본 라틴, 라틴-1, 문장부호, 화살표, 수학 기호, 상자 그리기, 도형                                                  | `charsets.ts`의 블록                                                       |

Noto Sans KR의 "나머지 음절" 파일은 현대 한글 음절 블록(U+AC00–D7A3)에서 상용 파일에 들어간 것을 뺀 8,822자다(`families.ts`의 `more`).

한자 목록은 `derive-charsets.ts`가 Unihan 18.0.0의 `kJoyoKanji`, `kTGH`와 Big5 바이트 범위에서, 기호 목록과 KS X 1001의 한글 목록은 각 국가 문자 집합의 해당 영역을 플랫폼 디코더로 풀어서 만든다.

**라틴 글꼴이 가진 글자를 어디서 빼는가**(`families.ts`의 `partition`, 테스트는 `tests/tools/fonts.test.ts`의 "the files of a family" 4건):

- 스택에서 라틴 family 뒤에 오는 family(SC, TC)는 Noto Sans(라틴)가 가진 글자를 모든 파일에서 뺀다. 그 글자는 CJK family에 오지 않기 때문이다. 전과 같고, SC·TC의 파일은 바이트까지 전과 같다.
- 자기 언어에서 스택 맨 앞에 서는 family(KR, JP: `leads`)의 **상용 파일**은 정의가 이름 붙인 글자를 라틴 family가 가졌더라도 그대로 갖는다. 한국어 화면에서는 KR 글꼴이 먼저 물어지므로, 거기 없는 글자만 Noto Sans로 넘어간다. KS X 1001과 JIS X 0208의 기호에 든 그리스·키릴 글자, 화살표, 단위 기호도 이 규칙으로 KR·JP의 상용 파일에 들어갔다(측정 때의 가정보다 글자가 353자 많다: 3,499 → 3,852). 나머지 음절 파일과 잔여 파일에서는 전처럼 뺀다(잔여 파일의 글자 수와 바이트가 전과 같다).
- 영어 화면에서는 Noto Sans가 앞이므로 KR·JP 상용 파일의 라틴 글자는 쓰이지 않는다.

파일 크기(2026-11-10 20:00 KST):

| 파일                            | 글자 수 | 바이트     |
| ------------------------------- | ------- | ---------- |
| `noto-sans-common.woff2`        | 779     | 112,876    |
| `noto-sans-rest.woff2`          | 2,315   | 426,924    |
| `noto-sans-kr-common.woff2`     | 3,852   | 517,728    |
| `noto-sans-kr-syllables.woff2`  | 8,822   | 775,336    |
| `noto-sans-kr-rest.woff2`       | 10,324  | 2,674,780  |
| `noto-sans-jp-common.woff2`     | 3,060   | 843,688    |
| `noto-sans-jp-rest.woff2`       | 13,484  | 3,534,452  |
| `noto-sans-sc-common.woff2`     | 4,228   | 1,143,144  |
| `noto-sans-sc-rest.woff2`       | 26,133  | 6,842,216  |
| `noto-sans-tc-common.woff2`     | 5,932   | 1,693,848  |
| `noto-sans-tc-rest.woff2`       | 14,284  | 4,017,412  |
| `d2coding-regular-common.woff2` | 809     | 28,712     |
| `d2coding-regular-rest.woff2`   | 19,127  | 1,443,768  |
| `d2coding-bold-common.woff2`    | 809     | 29,456     |
| `d2coding-bold-rest.woff2`      | 19,127  | 1,538,192  |
| 합계                            |         | 25,622,532 |

Noto 계열은 가변 글꼴(wght 축)이라 굵기별 파일이 없다. Noto Sans 원본에는 wdth 축도 있어 100으로 고정했다. D2Coding은 가변 빌드가 없어 Regular와 Bold가 따로다. KR의 규칙 CSS는 파일이 셋이 되어 87,765 → 151,944바이트(압축 전)로 늘었다.

**첫 화면의 글꼴 요청 수와 바이트**(Chromium headless, 기기에 Noto 없음, 2026-11-10 20:02 KST). `e2e/fonts.spec.ts`가 빌드 결과(`dist/`)의 스타일시트만으로 그린, 그 언어의 평범한 글 한 문단(글자·숫자·날짜·말줄임표·따옴표·화살표, 보통과 굵은 글씨)이 받는 글꼴 파일을 세고 고정한다. `pnpm --filter web run e2e`가 수치를 찍는다.

| 로케일                             | 요청 | 바이트    | 파일                                            |
| ---------------------------------- | ---- | --------- | ----------------------------------------------- |
| 영어                               | 1    | 112,876   | `noto-sans-common`                              |
| 한국어                             | 1    | 517,728   | `noto-sans-kr-common`                           |
| 한국어 + KS X 1001 밖의 음절(`똠`) | 2    | 1,293,064 | `noto-sans-kr-common`, `noto-sans-kr-syllables` |
| 일본어                             | 1    | 843,688   | `noto-sans-jp-common`                           |
| 중국어 간체                        | 2    | 1,256,020 | `noto-sans-common`, `noto-sans-sc-common`       |
| 중국어 번체                        | 2    | 1,806,724 | `noto-sans-common`, `noto-sans-tc-common`       |

한글 주석이 든 코드 블록이 있으면 D2Coding 2건이 더해진다(글꼴을 바꾼 뒤에 다시 재지는 않았다).

구현하면서 확인한 것:

- **한 family의 모든 파일은 `font-weight` 기술자가 같아야 한다.** 처음에는 상용 파일에 굵기별 규칙 넷, 잔여 파일에 `400 700` 범위 규칙 하나를 썼다. Chromium은 기술자가 가장 잘 맞는 규칙 묶음을 먼저 고른 뒤에야 `unicode-range`를 보므로, 상용 파일의 규칙이 통째로 무시되어 라틴 글자가 시스템 글꼴로 그려졌다(`CSS.getPlatformFontsForNode`로 확인). 지금은 모든 파일이 `100 599`와 `600 900` 두 규칙을 갖는다.
- **규칙이 둘인 이유는 `local()`이다.** 기기에 굵기별 파일로 설치된 Noto가 있으면 규칙 하나로는 Regular만 가리킬 수 있어 굵은 글자가 Regular를 뭉갠 모양이 된다. 600 이상은 설치된 Bold를 쓰게 했다. 서버에서 받는 가변 파일은 두 구간 어디서든 요청한 굵기를 낸다. 같은 파일을 두 규칙이 가리키므로 두 번째 굵기가 처음 나올 때 요청이 한 번 더 생기는데, 디스크 캐시에서 오고 전송은 0바이트다(CDP로 확인).
- **`unicode-range`는 파일끼리 겹치지 않게 정확히 적는다.** 넓은 범위로 겹쳐 두면 Chromium이 아직 못 받은 규칙을 건너뛰면서 다음 규칙의 파일도 함께 받는다. 정확히 적은 덕에, 한국어 화면에 KR 글꼴에 없는 한자(龘)가 나오면 KR 잔여 파일(2.7MB)을 건너뛰고 그 글자가 있는 SC 잔여 파일로 바로 간다.
- **그 값으로 규칙 CSS가 크다**(family당 압축 전 88–187KB, brotli 9–19KB). 그래서 라틴 family의 규칙만 주 스타일시트에 넣고, 나머지는 `src/lib/fonts.ts`가 따로 받는다: 현재 언어의 family는 바로, 다른 family와 D2Coding은 브라우저가 한가할 때.
- **기호는 국가 문자 집합 기준으로 상용 파일에 넣는다.** 넣기 전에는 `※`나 `→` 하나가 KR 잔여 파일을 받게 했다.

확인하지 못한 것:

- `local()`에 적은 이름(`Noto Sans KR`, `NotoSansKR-Regular`, `Noto Sans CJK KR Regular`, `NotoSansCJKkr-Regular` 등)이 실제 기기(Android, 리눅스 배포판, 글꼴을 직접 설치한 Windows·macOS)에서 맞는지. 이 기기에는 Noto가 없어 "없을 때 내려받는다" 쪽만 확인했다.
- Firefox와 Safari에서의 동작. Chromium headless만 돌렸다.
- 설치된 글꼴이 굵기별 파일일 때 500(Medium)은 Regular로, 600(SemiBold)은 Bold로 그려진다(설계상 그렇고, 실제 화면은 보지 못했다).

## 제3자 라이선스 (`tools/licenses`)

허용 목록은 따로 두지 않고 루트 `deny.toml`의 `[licenses] allow`를 읽는다(`policy.ts`). 검사는 둘이다.

1. **설치된 전체**(`pnpm --filter web run licenses`, `check`에 포함): `pnpm licenses list --json`의 모든 패키지. 허용 목록에 없으면 `exceptions.json`의 `tooling`에 이름·라이선스·사유가 있어야 한다. 맞는 패키지가 없는 예외는 오류다.
2. **번들에 들어간 것**(빌드 때, `notices.ts`): Vite의 라이선스 보고(`build.license`)에 나온 패키지 + `app.css`가 가져오는 CSS 패키지 + 도구가 소스에 써 넣은 코드의 출처(`vendored`: shadcn-svelte, paraglide). 예외 없이 허용 목록만 적용하고, 통과하면 `static/THIRD_PARTY_NOTICES.txt`를 쓴다.

`exceptions.json`의 갈래는 다섯이다: `tooling`(빌드 기계에서만 도는 도구), `stated`(라이선스 필드가 없어 패키지 안의 라이선스 파일로 정한 것), `notShipped`(설치되지만 빌드가 산출물에서 잘라 내는 것), `vendored`(도구가 소스에 써 넣은 코드의 출처), `derived`. 항목마다 이름·라이선스·사유가 있다.

결정이 필요한 것:

- **글꼴은 OFL-1.1인데 `deny.toml`의 목록에 없다.** Rust 크레이트 가운데 그 라이선스가 없어서다. 지금은 `notices.ts`가 글꼴 파일에 한해 OFL-1.1만 받는다(다른 라이선스의 글꼴은 빌드 실패). `deny.toml`에 `OFL-1.1`을 넣어 한 목록으로 만들지는 서버 쪽의 파일이라 손대지 않았다.
- **서버 쪽 `NOTICE`와 합치는 일은 하지 않았다.** 루트 `NOTICE`는 서버 쪽이 `cargo about`으로 만드는 파일이다. 웹 고지는 빌드 산출물로만 나온다. 패키징 단계(Docker 이미지)에서 둘을 이어 붙이는 것이 자연스럽다.

## 이름 검사

웹앱에 따로 두는 이름 검사는 없다. 저장소 전체의 `cargo xtask check` 이름 단계가 `apps/web` 아래 파일까지 본다: 두지 않는 낱말과 옛 식별자는 `xtask/src/banned.rs`, 워크스페이스의 옛 이름으로 된 식별자와 경로는 `xtask/src/names.rs`. 예외 목록은 없다.

## 순수 로직 (`src/lib/logic`)

계획의 W1이다. 화면 틀이나 상태 라이브러리에 기대지 않고, `window`나 `document`를 읽는 함수도 없다. 번역이 필요한 함수는 메시지를, 날짜 문구가 필요한 함수는 로케일(과 현재 시각)을 인자로 받는다.

속성 종류는 `PropertyTemplate.type`으로 구별하고, 종류마다의 동작은 `PROPERTY_BEHAVIORS` 표 한 곳에 있다(`canGroup`, `canFilter`, `filterValueType`, `isReadOnly`, `calculationOptions`, `displayValue`, `exportValue`, `dateRange`).

### 와이어 형식에 기대는 곳: `wire.ts` 하나

서버의 와이어 설계가 바뀔 때 고칠 곳을 한 파일로 모았다. `wire.ts`만 아는 것:

- 블록·카드·뷰·보드의 타입과, 빠진 값을 채워 만드는 함수(`createBlock`, `createCard`, `createBoardView`, `createBoard`, `normalizeBlock` 등)
- 블록의 형식 없는 `fields` 안의 내용. 다른 모듈은 `cardProperty`, `cardProperties`, `setCardProperty`, `cardContentOrder`, `viewQuery`, `viewGrouping`, `remapBlockReferences` 같은 함수로만 읽고 쓴다
- 날짜 속성 값과 날짜 필터 값의 저장 형식(`parseDateValue`, `formatDateValue`, `parseFilterDate`, `formatFilterDate`, `plainPropertyValue`). 로직은 `dates.ts`의 내부 표현만 다루고, 와이어 값(달력 날짜 문자열, 시각의 UTC 밀리초 정수)은 여기서만 변환한다
- 식별자 만들기(`createId`: 종류 한 글자 + z-base-32 26자)
- 패치의 모양(`createPatchesFromBlocks`, `createPatchesFromContainers`, `createCardPropertiesPatches`, `createPatchesFromContainersAndBlocks`)

`tests/logic/wire-boundary.test.ts`가 이 규칙을 검사한다: `wire.ts`가 아닌 로직 모듈에 `.fields`나 `JSON.parse`·`JSON.stringify`가 나오면 실패하고, `wire*` 테스트와 픽스처(`support/factory.ts`)가 아닌 테스트가 `.fields`를 읽어도 실패한다.

경계 밖에 남은 것은 모든 블록이 최상위에 갖는 멤버다. 다른 모듈이 직접 읽는다: `id`, `title`, `type`, `containerId`, `parentId`, `createdBy`, `modifiedBy`, `createdAt`, `updatedAt`, `deletedAt`, `limited`. 보드에서는 속성 정의 목록(`PropertyTemplate[]`)을 인자로 받을 뿐 보드 객체를 받지 않는다. 이 이름들이 바뀌면 타입 정의가 `wire.ts`에 있으므로 컴파일러가 쓰는 곳을 전부 짚어 준다.

와이어가 바뀌면 사라지거나 달라질 것으로 보이는 부분: `createId`(ID를 서버가 만들게 되면 필요 없다), 날짜 값의 두 형식, `createAttachmentBlock`의 옛 이름 `attachmentId`, `Board.channelId`.

### 정렬, 필터, 검색, 계산의 규칙 (Q1–Q16)

번호는 규칙에 붙인 이름이다. 코드와 테스트의 주석이 이 번호로 부른다. 테스트는 모두 `tests/logic/` 아래다.

| #   | 무엇                                       | 지금의 동작                                                                                                                                                                                                                   | 고정하는 테스트                                                                                           |
| --- | ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Q1  | 수동 순서에서 위치 없는 카드               | 위치 없는 카드는 항상 위치 있는 카드 뒤. 그 안에서는 만든 순서(같은 밀리초면 ID 순). 비교자는 순서 관계다                                                                                                                     | `sort.test.ts` "manual order", `sort-properties.test.ts`(속성 기반)                                       |
| Q2  | 텍스트 필터의 대소문자                     | 제목과 모든 속성 종류에서 대소문자를 가리지 않는다. 비교는 `foldCase`(NFKC 정규화 뒤 소문자)로 하며 기기나 화면 언어에 따라 달라지지 않는다. 전각 영문과 풀어 쓴 한글 자모도 같게 본다. 값이 목록이면 항목 하나가 맞으면 된다 | `filter-rules.test.ts` "text conditions do not look at case"                                              |
| Q3  | 검색의 일치 방식                           | 모든 종류가 부분 일치. 대소문자 처리는 Q2와 같은 `foldCase`                                                                                                                                                                   | `views.test.ts` "search"                                                                                  |
| Q4  | 사람별 그룹에서 사람 없는 카드             | ID가 빈 문자열인 "사람 없음" 그룹이 된다. 이름은 호출부가 주는 문구(선택 속성의 "값 없음" 열과 같은 함수). 맨 앞에 오고, 그런 카드가 없으면 그룹도 없다. 숨김 목록에 `''`가 있으면 다른 그룹처럼 숨는다                       | `views.test.ts` "groups of a board view"                                                                  |
| Q5  | 날짜의 `is` 필터                           | 달력 날짜로 비교한다. 시각이 붙은 값은 보는 사람의 시간대에서 그 시각이 속한 날로 본다. 범위는 양 끝을 포함한 모든 날에 맞는다                                                                                                | `filter-rules.test.ts` "a date property is compared as a calendar day"                                    |
| Q6  | 정렬 옵션이 둘 이상일 때                   | 적힌 순서대로 모두 적용한다. 첫 옵션이 순서를 정하고, 같으면 다음 옵션이 정한다. 모든 옵션에서 같으면 제목, 만든 순서, ID 순. 옵션마다 방향을 따로 뒤집는다. 없어진 속성의 옵션은 건너뛴다                                    | `sort.test.ts` "several sort options", `sort-properties.test.ts`                                          |
| Q7  | 파일 이름 정리의 금지 문자                 | 전부 지운다                                                                                                                                                                                                                   | `values.test.ts` "text"                                                                                   |
| Q8  | base path가 있을 때 `buildURL`의 상대 결과 | 항상 사이트 루트에서 시작한다(`/base/test`)                                                                                                                                                                                   | `utils.test.ts`                                                                                           |
| Q9  | 체크박스 계산에서 "체크"                   | 저장된 값이 `true`일 때만 체크다                                                                                                                                                                                              | `calculations.test.ts`, `edges.test.ts` "checkbox calculations"                                           |
| Q10 | 숫자만 적힌 날짜 값                        | 날짜 값의 모양이 아니므로 읽지 않는다. 필터·표시·정렬·계산 모두 빈 값으로 본다                                                                                                                                                | `wire-shapes.test.ts`, `filter-rules.test.ts` "does not read what is not a date value"                    |
| Q11 | 날짜만 있는 값의 저장                      | 날짜만 있는 값은 내부에서도 와이어에서도 달력 날짜(`2026-03-05`)다. 시각으로 꾸미지 않는다(아래 "날짜 속성 값의 와이어 모양")                                                                                                 | `wire-shapes.test.ts` "the stored value of a date property", `values.test.ts` "calendar dates"            |
| Q12 | CSV의 머리글 행                            | 머리글도 다른 텍스트 칸처럼 감싸고 따옴표를 겹쳐 쓴다                                                                                                                                                                         | `views.test.ts` "CSV rows"                                                                                |
| Q13 | 필터의 속성을 사람 종류로 바꿨을 때의 조건 | 사람 종류는 선택지 종류와 같은 조건 넷을 유지한다                                                                                                                                                                             | `views.test.ts` "the condition of a filter after its property changed"                                    |
| Q14 | 버려도 되는 변경의 체크포인트              | 앞선 변경이 있으면 늘 그 체크포인트를 잇는다. 맨 처음이면 0                                                                                                                                                                   | `edges.test.ts` "undo manager"                                                                            |
| Q15 | 카드가 없을 때의 체크 비율                 | 빈 문자열                                                                                                                                                                                                                     | `edges.test.ts` "checkbox calculations"                                                                   |
| Q16 | 생성·수정 시각 필터의 "하루"               | 보는 사람의 시간대의 달력 날짜다. 서울에서 7일 03:00에 만든 카드는 "7일"이다. 시간대는 필수 인자(`FilterContext.timeZone`, IANA 이름)다. 기기의 시간대로 떨어지는 기본값은 없다                                               | `filter-rules.test.ts` "the day a card was created is the day on the reader's calendar", `filter.test.ts` |

그 밖의 규칙:

- **제목 없는 카드와 값 없는 카드는 정렬 방향과 무관하게 항상 뒤다.**
- **사람 속성으로 정렬하면 사용자 이름순이다.**
- **정렬의 글자 순서는 `SortContext.locale`의 `Intl.Collator`를 따른다.** 언어도 시간대처럼 필수 인자다(기기의 언어로 떨어지지 않는다).
- **체크박스의 "값 있음" 계열 계산**(`countValue`, `countEmpty` 등)은 `false`가 저장돼 있으면 값이 있는 것으로 센다. 체크 여부를 묻는 네 계산만 Q9의 규칙을 따른다.
- **날짜 속성의 earliest·latest·dateRange**는 달력 날짜로 계산한다. `dateRange`는 양 끝을 포함한 날 수다(5일–12일은 "8 days").
- **실행 취소 관리자는 `undo`나 `redo`가 예외를 던져도 멈추지 않는다.** `edges.test.ts`의 다섯 건이 고정한다: 실패한 취소를 다시 시도할 수 있다, 실패한 다시 실행도 같다, 실패 뒤에도 새 변경이 기록된다, 묶음 취소가 실패한 자리에서 멈춘다, 취소 중에 생긴 변경은 기록하지 않는다.

### 날짜: 내부 표현과 규칙

- **달력 날짜**(`CalendarDate`)는 `YYYY-MM-DD` 문자열이다. 시각도 시간대도 없다. 문자열 비교가 곧 날짜 비교다.
- **날짜 값**(`DateValue`)은 `from`과 선택적인 `to`이고, 각 끝은 달력 날짜(`{ date }`)이거나 시각(`{ instant }`)이다. 시각은 날짜에 시간이 붙은 경우에만 쓴다.
- **"하루"는 보는 사람의 시간대 기준이다.** 시각(카드를 만든 때, 시간이 붙은 날짜 값)이 어느 날에 속하는지는 `dateOfInstant(instant, timeZone)`이 정한다. 날짜만 있는 값은 누구에게나 같은 날이다.
- 비교: `is`는 그 날이 값이 덮는 날들(범위면 양 끝 포함) 안에 있는가, `isBefore`는 값의 첫날이 그 날보다 앞인가, `isAfter`는 값의 마지막 날이 그 날보다 뒤인가.
- 정렬: 첫날 순. 같은 날이면 시간 없는 값이 시간 있는 값보다 앞, 시간끼리는 이른 순.
- 표시: 날짜만 있는 값은 시간대와 무관하게 같은 글자(`March 05`)로 나온다.
- 시각 사이의 범위(`dateRange`를 생성·수정 시각에 쓴 경우)는 지나간 시간이다(72시간이면 "3 days"). 달력 날 수로 세는 것은 날짜 속성뿐이다.

### 보는 사람의 시간대: 계정 설정

2026-11-10 19:16 KST에 정했다. "보는 사람의 시간대"는 기기에서 직접 읽지 않는다. 계정 설정의 값이고, 로직에는 인자로 들어온다.

- **`lib/logic`의 모든 날짜·시각 함수는 시간대를 필수 인자로 받는다**(IANA 이름, 예: `Asia/Seoul`). `dateOfInstant(instant, timeZone)`, `startOfDay(date, timeZone)`, `endOfDay(date, timeZone)`, `daysOf(value, timeZone)`, `comparePoints(a, b, timeZone)`, 그리고 `FilterContext.timeZone`, `SortContext.timeZone`, `DisplayContext.timeZone`. 기본값이 없으므로 빠뜨리면 타입 오류다. 시각은 `Date` 객체가 아니라 에포크 밀리초(숫자)로 주고받는다.
- **변환은 `Intl`과 `@internationalized/date`로 한다.** 시각 → 날짜는 `Intl.DateTimeFormat`(`timeZone` 지정)의 `formatToParts`, 날짜 → 그날이 시작하는 시각은 `@internationalized/date`의 `toZoned`.
- **린트 규칙 `fvoci/no-device-time`**(`tools/lint/layers.ts`, `src/lib/logic/**`에 적용, `check`에 포함)이 다음을 막는다: `getHours`·`getDate`·`getDay`·`getMonth`·`getFullYear`·`getTimezoneOffset`·`setHours`·`toLocaleDateString` 등 기기 시각으로 답하는 `Date` 메서드, `new Date(연, 월, …)`과 `new Date('글자')`, `timeZone` 옵션 없는 `Intl.DateTimeFormat`, `resolvedOptions().timeZone`. `getUTC…`, `Date.UTC`, `Date.now()`, `getTime()`은 된다. 규칙의 테스트는 `tests/tools/layers.test.ts`(4건).
- **설정**(`src/lib/logic/regional.ts`): `RegionalSettings`는 세 값이다.

  | 값          | 종류                                           | 기본     | "자동"의 뜻                                                                                          |
  | ----------- | ---------------------------------------------- | -------- | ---------------------------------------------------------------------------------------------------- |
  | `timeZone`  | `'auto'` 또는 IANA 이름                        | `'auto'` | 브라우저의 시간대                                                                                    |
  | `weekStart` | `'auto'`, `'sunday'`, `'monday'`, `'saturday'` | `'auto'` | 화면 언어·지역의 관습(`Intl.Locale`의 주 정보. 한국어·미국 영어는 일요일, 영국 영어·독일어는 월요일) |
  | `hourCycle` | `'auto'`, `'h12'`, `'h23'`                     | `'auto'` | 화면 언어의 관습(한국어·미국 영어는 12시간, 독일어·일본어는 24시간)                                  |

  값의 종류는 서버의 세 설정과 같다. 푼 뒤의 `weekStart`는 여전히 ISO 8601의 요일 번호(1 월요일 – 7 일요일)다. "자동"이 풀리는 요일은 로케일에 따라 셋 밖일 수 있기 때문이다. `readRegionalSettings(받은 값)`은 서버의 응답을 설정으로 읽고, 모르는 값은 `'auto'`로 읽는다.

  `resolveRegional(settings, locale, browserTimeZone)`이 "자동"을 풀어 `{ timeZone, weekStart, hourCycle }`을 준다. 고정한 시간대를 런타임이 모르면(이름이 바뀐 시간대) 브라우저의 것을, 그것도 못 쓰면 UTC를 쓴다. `weekStart`는 달력을 그리는 `DateProperty.svelte`와 `FilterDate.svelte`가 쓴다. `hourCycle`은 `DisplayContext.hourCycle`로 `displayDateTime`에 들어간다. 세 값을 고르는 화면은 설정 대화상자다.

- **브라우저의 시간대를 읽는 곳은 `src/lib/regional.ts` 하나다**(`browserTimeZone()`). 같은 파일의 `regional(locale)`이 설정을 풀어서 주고, `setRegionalSettings`로 설정을 바꾼다. 작업 공간 저장소(`src/lib/state/workspace.svelte.ts`)가 `Backend`에서 받은 설정으로 그 함수를 부른다. `src` 전체에서 기기의 시간대를 묻는 표현(`resolvedOptions().timeZone`, `getLocalTimeZone`, `getTimezoneOffset`)이 이 파일에만 있다는 것을 `tests/tools/layers.test.ts`의 한 건이 고정한다. 그러려고 shadcn 달력의 머리글 두 파일도 고쳤다(`src/lib/ui/README.md`의 표).
- **테스트**(`tests/logic/time-zones.test.ts` 20건, `regional.test.ts` 9건): 서머타임으로 하루가 23시간·25시간인 날(뉴욕 2026-03-08, 2026-11-01)의 시작과 끝, 그날의 마지막 순간과 다음 날의 첫 순간이 필터에서 갈리는 것, 자정이 건너뛰어지는 날(아바나), 30분만 바뀌는 곳(로드하우), 날짜 변경선 양쪽(키리티마티 UTC+14, 파고파고 UTC−11, `Etc/GMT+12`)에서 같은 시각이 세 날짜로 갈리는 것과 같은 날짜가 26시간 차이로 시작하는 것, 달력에서 빠진 날(사모아 2011-12-30), "올해"가 보는 사람의 달력으로 정해지는 것. 날짜만 있는 값은 열 개 시간대에서 덮는 날·표시·필터·정렬이 모두 같다는 것(속성 기반 300회 포함). 임의의 시각과 시간대에 대해 "시각은 자기 날의 시작과 끝 사이에 있다"(속성 기반 500회).
- 서버 쪽: 설정은 `GET`·`PATCH /users/me/settings`로 오가고 응답은 늘 세 멤버를 모두 싣는다. 브라우저가 아는 시간대 이름을 서버가 모르면(시간대 자료의 판 차이) 서버가 거절한다. 그때 저장소가 되돌리고 사용자에게 알린다.
- 확인하지 못한 것: 브라우저가 가진 시간대 자료(ICU)의 판이 서로 다를 때의 차이, `Intl.Locale`의 주 정보가 없는 오래된 브라우저(그때는 월요일로 떨어진다).

### 날짜 속성 값의 와이어 모양

- 날짜 속성의 값은 문자열에 담긴 JSON이 아니라 **JSON 객체**다. 달력 날짜는 `"2026-03-05"` 문자열, 시각은 1970-01-01 00:00 UTC부터의 **밀리초 정수**다. 날짜인지 시각인지는 JSON의 타입이 말한다.
  - 하루: `{"from": "2026-03-05"}`, 범위: `{"from": "2026-03-05", "to": "2026-03-09"}`(`to`는 그날을 포함), 시각: `{"from": 1772703000000}`.
  - `from`은 필수, `to`는 선택. 둘은 같은 종류여야 하고 `to`는 `from`보다 앞설 수 없다. 같으면 `to` 없이 적는다. `includeTime`과 `timeZone` 멤버는 없다.
  - 날짜 필터의 값도 `"2026-03-05"`다.
- `wire.ts`: `PropertyValue`의 `StoredDateValue`(`{ from: string | number; to?: string | number }`). `parseDateValue(stored: unknown)`는 객체를 받고, `formatDateValue(value)`는 객체(빈 값이면 `undefined`)를 준다. `parseFilterDate`·`formatFilterDate`는 달력 날짜 문자열 그대로다. 새 함수 `plainPropertyValue`는 저장 값을 글자나 글자 목록으로 준다(날짜 값은 세고 비교하기 위한 키 `2026-03-05/2026-03-09`가 된다). 날짜가 아닌 종류의 로직(계산의 값 세기, 정렬, 검색, 텍스트 필터)이 이것을 거친다.
- **읽을 때의 규칙**: 서버가 검증하는 것과 같은 조건을 만족하지 않으면 빈 값이다(없는 날, 연도 0, 소수나 음수, 9999년을 넘는 정수, 종류가 다른 두 끝, 거꾸로 된 범위, 그 밖의 타입). 문자열에 담긴 JSON(`"{\"from\":1772712000000}"`)은 읽지 않는다.
- **쓸 때의 규칙**: 두 끝이 같으면 `from`만, 거꾸로 된 범위는 뒤집어 쓴다. 날짜와 시각이 섞인 값(편집기가 만들지 않는다)은 `from`만 쓴다.
- 내부 표현은 `dates.ts`의 `CalendarDate`, `DatePoint`, `DateValue`다. `calendarDate`와 `parseCalendarDate`는 서버가 받는 0001–9999년 전체를 다룬다(`Date.UTC`는 1–99년을 1901–1999년으로 읽는다).
- 테스트: `tests/logic/wire-shapes.test.ts`의 "the stored value of a date property"가 읽기·쓰기·왕복·거절을 고정한다.

### 테스트

- 새로 쓴 테스트는 정렬, 검색, 그룹, 속성 표, CSV, 날짜 변환, 서식을 덮는다.
- **시간대.** `test` 스크립트가 전체 단위 테스트를 프로세스의 `TZ=UTC`, `Asia/Seoul`, `Pacific/Kiritimati`(UTC+14)로 세 번 돌린다. 로직은 기기의 시간대를 묻지 않으므로, 이 실행은 "프로세스의 TZ가 무엇이든 결과가 같다"를 확인하는 용도다. 시간대에 따라 달라져야 하는 동작은 테스트가 시간대를 인자로 넘겨 검사한다.
- **속성 기반 테스트**(`sort-properties.test.ts`, fast-check 4.10.2). 생성한 카드와 수동 순서, 정렬 옵션 조합에 대해 두 가지를 각 300회 검사한다. 하나는 비교자가 순서 관계인지다(자기 자신과는 0, 뒤집으면 부호가 반대, 추이적, 서로 다른 카드는 0이 아님). 다른 하나는 정렬 결과가 입력 순서에 좌우되지 않는지다.

## 화면과 데이터의 경계

화면은 `src/lib/state`의 인터페이스(`Backend`)만 부른다. 그 뒤에는 서버를 부르는 구현(`src/lib/api`)과 브라우저 안의 메모리 구현(`src/lib/state/memory`, 내장 템플릿으로 채운 작업 공간)이 있고, 어느 쪽인지는 `src/app/backend.ts`가 고른다. 서버 없이 띄운 빌드(`tools/preview.ts`, `e2e`)는 메모리 구현으로 돈다. 화면이 서버를 직접 부르지 못하게 하는 것은 린트(`fvoci/no-direct-server`)가 본다.

| 무엇                                                                          | 어디                                                                                                               |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| 데이터의 경계, 메모리 구현, 저장소                                            | `src/lib/state/`(`backend.ts`, `memory/`, `workspace.svelte.ts`, `board.svelte.ts`, `card.svelte.ts`)              |
| 서버의 모양과 화면의 모양 사이                                                | `src/lib/api/`(`contract.ts`는 `contracts/openapi.json`에서 만든다, `convert.ts`)                                  |
| 층의 순서와 린트                                                              | `tools/lint/layers.ts`(`fvoci/layer-order`, `fvoci/no-direct-server`), `src/lib/README.md`                         |
| 표시 설정(밝기, 밀도, 카드 여는 방식, 진동), 기기 설정을 따르는 것            | `src/lib/logic/display.ts`, `src/lib/display.svelte.ts`, `src/lib/theme.svelte.ts`, `index.html`의 인라인 스크립트 |
| 작업 공간의 틀: 사이드바(목록·서랍·띠), 위쪽 막대, 설정 대화상자, 개발용 표시 | `src/lib/features/shell/`                                                                                          |
| 뷰 머리글: 뷰 탭과 메뉴, 속성, 그룹, 필터, 정렬, 검색, 접힌 줄                | `src/lib/features/view-header/`                                                                                    |
| 칸반과 갤러리, 카드의 겉                                                      | `src/lib/features/views/kanban/`, `gallery/`, `src/lib/components/card-face/`                                      |
| 테이블, 캘린더, 간트                                                          | `src/lib/features/views/table/`, `calendar/`, `gantt/`                                                             |
| 카드 상세: 세 가지 틀, 속성과 그 정의, 본문, 첨부, 댓글                       | `src/lib/features/card/`                                                                                           |
| 편집기, 읽기 전용 표시기, 수식(KaTeX), 다이어그램(Mermaid), 이모지 목록(CLDR) | `src/lib/editor/`                                                                                                  |
| 속성 편집기                                                                   | `src/lib/components/properties/`, `stories/properties/`, `tests/ui/properties/`                                    |
| 드래그                                                                        | `src/lib/drag/`, `src/lib/logic/drop-target.ts`                                                                    |
| 조립: 주소, 보드 페이지, 카드 대화상자                                        | `src/app/`                                                                                                         |
| 빌드의 `.zst`                                                                 | `tools/build/`, `docs/build-notes.md`                                                                              |
| 내장 템플릿                                                                   | 저장소 루트의 `crates/server/assets/templates/`(그 폴더의 `README.md`)                                             |

화면의 그림은 커밋하지 않는다. `FVOCI_SCREENS=1`로 찍으면 `target/screens/<화면>/`(저장소의 맨 위, `e2e/screens.ts`)에 생긴다: `FVOCI_SCREENS=1 pnpm exec playwright test e2e/<화면>.spec.ts -g "pictures|screens|screenshots"`(`apps/web`에서, `build` 뒤에). 조립된 앱을 띄워 보는 법은 `docs/handoff.md`의 "화면을 띄우는 법과 포트"에 있다.

### 기기에 남는 것

기기에 남는 것은 `localStorage`의 다섯 키뿐이다. 읽고 쓸 때마다 try/catch로 감싸고, 없어도 화면은 그대로 그려진다.

| 키                      | 무엇                                                                           |
| ----------------------- | ------------------------------------------------------------------------------ |
| `fvoci:display`         | 밝기·밀도·카드 여는 방식·진동. 첫 그리기 전에 `index.html`의 스크립트가 읽는다 |
| `fvoci:memory-settings` | 메모리 구현이 들고 있는 나머지 설정                                            |
| `fvoci:locale`          | 계정 설정 `language`의 사본: 로그인 전과 첫 그리기의 언어. `auto`면 없다       |
| `fvoci:sidebar`         | 접은 사이드바                                                                  |
| `fvoci:dev`             | 개발용 표시(`features/shell/DevReadout.svelte`: 창의 너비와 지금의 단계)       |

### 다시 겪을 수 있는 함정

- **놓자마자 재는 코드.** "움직임 줄이기"를 켠 기기에서 페이지는 모든 전환을 0.01ms로 줄인다. 다이어그램 라이브러리는 노드를 놓자마자 그림의 크기를 재므로, 그리는 자리에서는 전환을 끈다. 같은 함정이 "놓자마자 재는" 다른 코드에도 있을 수 있다.
- **방문 기록.** 닫히는 시트는 자기 칸을 `history.back()`으로 돌려주는데 이것은 늦게 일어난다. 그사이에 넣은 칸을 브라우저가 엉뚱한 자리에서 셀 때가 있어, 방문 기록에 쓰는 것(시트의 칸, 라우터의 주소, 카드를 닫는 한 걸음)은 우리가 낸 "뒤로"가 도착할 때까지 기다린다(`lib/layout/back.svelte.ts`의 `afterOwnSteps`).
- **수식의 글꼴.** 가장 작은 글꼴이 스타일시트에 `data:` 주소로 박히면 정책의 `font-src 'self'`에 걸린다. 그래서 `assetsInlineLimit: 0`이다.

### 본문 읽기 전용 표시기

편집기(Tiptap) 없이 문서를 DOM으로 그린다(`src/lib/editor/render.ts`, `BodyView.svelte`, `drawing.ts`). 허용 목록의 노드와 마크를 모두 그리고, 검증을 통과하지 못한 문서는 그리지 않으며, 링크의 `rel`·`target`은 그릴 때 붙인다. 글자는 텍스트 노드로만 넣는다.

- 편집기가 그린 것과의 비교(`tests/ui/body-compare.test.ts`): 템플릿의 문서, 손으로 만든 문서(`tests/support/body-docs.ts`), 모든 노드와 마크가 든 문서(`tests/ui/fixtures/body-samples.ts`)에서 정규화한 DOM이 같다. 비교에서 빼는 것은 그 파일 머리에 적혀 있고, 한 번도 쓰이지 않는 항목이 생기면 테스트가 실패한다.
- 적대적인 다이어그램과 수식이 진짜 라이브러리를 거쳐 편집기와 읽기 전용 표시기에 닿는 길 전체는 `e2e/editor-attacks.spec.ts`가 본다. 다른 호스트에 아무것도 묻지 않고, 스크립트가 돌지 않고, 그린 것에 스크립트·링크·처리기·바깥 주소가 없다.

### 속성 편집기

`src/lib/components/properties/`의 `PropertyValue.svelte`가 속성 정의의 종류로 갈라 그린다. 쓰임은 둘이다: 표시(`mode: 'display'`)와 편집. 15종(text, number, select, multiSelect, date, person, multiPerson, checkbox, url, email, phone, createdTime, createdBy, updatedTime, updatedBy) 모두 표시와 편집(읽기 전용 넷은 표시)이 된다. 종류마다 Storybook 이야기와 상호작용 테스트(`tests/ui/properties/<종류>.test.ts`)가 있다.

- **날짜**: 범위와 시각을 포함한다. 시각 없는 값은 달력 날짜, 시각 있는 값은 UTC 밀리초다. 입력과 표시는 **계정의 시간대**(인자 `regional.timeZone`)로 한다. 기기의 시간대를 읽지 않는다는 것을 테스트가 고정한다. 서머타임으로 없는 시각은 건너뛴 만큼 뒤로, 두 번 있는 시각은 첫 번째로 읽는다. 순수 로직은 `src/lib/logic/date-entry.ts`.
- **좁은 화면에서 선택 목록과 날짜 선택기가 뜨는 방식**은 한 곳에 있다: `src/lib/components/picker-surface/PickerSurface.svelte`. "넓으면 팝오버, 좁으면 바닥 시트"이고, 어느 쪽인지는 인자 `presentation` 하나가 정한다.
- 선택지 색 10종의 토큰(`--option-*`)은 `app.css`에 있고 대비 검사에 들어 있다.
- 이메일과 숫자의 검증은 저장된 값이 아니라 입력한 값에 한다. 체크박스는 `'true'`만 체크로 읽는다. 모르는 사용자 ID는 "알 수 없는 사용자"로 보인다. 선택지의 이름을 바꿀 수 있다.

### 드래그

- 조정 값은 한 곳에 있다(`src/lib/drag/settings.ts`). 보드 밖에 놓으면 취소, 정렬된 보기에서는 열만 바뀌고 안내를 보인다. 길게 누르기가 성립할 때 진동한다. 열도 끈다(두 단계 정렬).
- 여러 장(`moveItems`, `selection`), 숨긴 열 같은 "놓을 곳"(`data-drag-zone`)은 기반과 단위 테스트가 있다.
- 시작 방식은 너비가 아니라 누름마다 포인터 종류(`pointerType`)로 갈린다. 창의 너비가 달라지거나 높이가 120px 넘게 달라지면 진행 중인 드래그를 취소한다.

### 화면 크기의 세 단계

좁은 화면과 넓은 화면 사이에 중간 단계가 있다(접는 기기, 태블릿). 단계는 기기가 아니라 쓸 수 있는 너비로 나누고, 부품은 자기가 놓인 칸의 너비를 본다. 경계 값은 `src/styles/app.css`의 `@theme`에 있고 상자의 너비를 컨테이너 쿼리로 본다.

- 창 크기가 바뀌어도 편집기의 글·커서·초점·열린 목록은 그대로다. 카드는 어느 단계에서나 대화상자 하나이고 클래스만 바뀐다(`features/card/frame.ts`).
- 직접 쓴 부품은 높이에 `dvh`를 쓴다(시트 `85dvh`, 목록 `45dvh`). 좌우와 아래의 안전 영역은 각자의 값으로 준다(`viewport-fit=cover`와 함께: 그 값이 없으면 `env(safe-area-inset-*)`가 모두 0이다).
- 제품 e2e에는 중간 크기 프로젝트 `middle`(626×890, 터치)이 있다. 이 크기들은 어림값이고 실제 기기에서 확인하지 않았다.

## 미결 사항

열려 있는 것이다.

| 항목                             | 내용                                                                                                                                |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| 중국어의 문장부호 글꼴           | 중국어 두 로케일은 라틴 family가 앞이라 문장부호가 Noto Sans의 모양이다(말줄임표가 아래에 붙는다). 전각 따옴표 때문에 그렇게 정했다 |
| 비 CJK 언어에서의 CJK 순서       | `lang`이 ko·ja·zh가 아니면 KR, JP, SC, TC 순이다. 영어 화면에 일본어 글이 나오면 한자가 한국어 자형으로 그려진다                    |
| 브랜드 색                        | 무채색 그대로                                                                                                                       |
| Noto Serif의 용도                | 도구만 지원한다(`--family noto-serif,noto-serif-kr`). 토큰에는 없다                                                                 |
| 이탤릭                           | Noto Sans Italic은 별도 파일이라 넣지 않았다. 지금은 브라우저가 기울여 그린다                                                       |
| `OFL-1.1`을 `deny.toml`에 넣을지 | 위 "제3자 라이선스"                                                                                                                 |
| 웹 고지와 서버 `NOTICE` 합치기   | 패키징 단계에서                                                                                                                     |
| 번들 크기 상한                   | 정하지 않았다. 위 "번들 크기"                                                                                                       |
