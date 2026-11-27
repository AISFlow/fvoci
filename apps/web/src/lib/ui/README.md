# 공용 UI (shadcn-svelte 생성물)

이 폴더의 컴포넌트는 shadcn-svelte CLI가 만든 **가져온 코드**다. 기반은 Bits UI, 아이콘은 `@lucide/svelte`, 스타일 프리셋은 `nova`, 기본색은 `neutral`이다. `$lib/hooks/is-mobile.svelte.ts`와 `$lib/utils.ts`도 같은 CLI의 산출물이다.

시각은 모두 KST다.

## 생성 명령 (2026-11-10 16:58 KST, shadcn-svelte 1.8.0)

```bash
# apps/web 에서. 초기화: components.json, src/lib/utils.ts, src/styles/app.css 의 변수
pnpm dlx shadcn-svelte@1.8.0 init --preset nova --no-deps-install --base-color neutral \
  --css src/styles/app.css --components-alias '$lib' --lib-alias '$lib' \
  --utils-alias '$lib/utils' --hooks-alias '$lib/hooks' --ui-alias '$lib/ui'

# 컴포넌트 23종
pnpm dlx shadcn-svelte@1.8.0 add button input textarea checkbox switch label select dialog \
  alert-dialog dropdown-menu popover tooltip sheet sidebar tabs collapsible separator badge \
  skeleton sonner calendar range-calendar command --yes --no-deps-install
```

`command`의 레지스트리 의존성으로 `input-group`이 함께 들어와 폴더는 24개다.

알아 둘 것:

- `init`은 `--preset`이 없으면 대화형 질문에서 멈춘다. 프리셋 이름은 `nova`, `vega`, `maia`, `lyra`, `mira`다(CLI 소스에서 확인).
- `init`과 `add` 모두 "덮어쓸까요" 같은 확인 질문을 한다. 표준 입력이 닫혀 있으면 아무것도 쓰지 않고 끝난다. 자동화할 때는 Enter(`\r`)를 몇 초 간격으로 넣어 준다.
- `init`이 `app.css`에 `@import "@fontsource-variable/geist"`와 `--font-sans: 'Geist Variable'`을 넣는다. 이 프로젝트의 글꼴은 Noto Sans와 D2Coding이므로 지웠다.
- 별칭은 `tsconfig.json`의 `paths`에서 읽는다. `svelte.config.js`에 `kit.alias`를 둘 필요가 없다(없이 `add`가 되는 것을 확인했다).

## CLI가 바꾼 의존성과 되돌린 내용

`--no-deps-install`을 줘도 CLI는 `package.json`의 판을 범위로 고쳐 쓴다(설치는 하지 않았고 잠금 파일은 그대로였다). 생성 직후 `package.json`을 비교해 고정 판으로 되돌렸다.

| 패키지                       | CLI가 쓴 값                  | 되돌린 값 | 이유                                                                                 |
| ---------------------------- | ---------------------------- | --------- | ------------------------------------------------------------------------------------ |
| `@lucide/svelte`             | `^1.71.0`                    | `1.70.0`  | 고정 판 유지. 1.71.0은 공개 24시간 미만이었다                                        |
| `tailwind-variants`          | `^3.3.0` (`init`은 `^4.0.2`) | `3.3.1`   | `add`가 요구한 범위 안의 최신이었다. 지금은 `4.0.2`(생성 코드 그대로, 클래스가 같다) |
| `tw-animate-css`             | `^1.4.0`                     | `1.4.0`   | 고정 판 유지                                                                         |
| `cn`                         | `^0.5.2` (새로 추가)         | `0.5.2`   | `$lib/utils.ts`가 `cn`을 이 패키지에서 다시 내보낸다                                 |
| `shadcn-svelte`              | `^1.8.0` (새로 추가)         | `1.8.0`   | `app.css`가 `shadcn-svelte/tailwind.css`(커스텀 변형 정의)를 가져온다                |
| `@fontsource-variable/geist` | `^5.3.0` (새로 추가)         | 제거      | 쓰지 않는 글꼴                                                                       |
| `mode-watcher`               | `^1.1.0` (새로 추가)         | 제거      | 아래 `sonner` 수정 참고                                                              |

## 손댄 곳 (2026-11-10 17:00–17:10 KST)

손댄 파일은 첫 줄에 `<!-- fvoci: … -->` 주석이 있다. 다시 생성하면 이 표를 따라 같은 수정을 되풀이한다. 변경 내역은 생성 직후 사본과 `diff -r`로 비교했다(23개 파일, 뒤에 2개를 더 고쳤다).

| #   | 파일                                                                                                                                                                                                                                                                                       | 수정                                                                                                        | 이유                                                                                                                                                                                                                                                                        |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `command/command-input.svelte`                                                                                                                                                                                                                                                             | `text-sm` → `text-base md:text-sm`                                                                          | 입력 칸 글자가 16px 미만이면 iOS가 초점 때 화면을 확대한다. `input`, `textarea`는 생성물이 이미 `text-base md:text-sm`이었고, 이 칸만 `text-sm`으로 덮어쓰고 있었다. `select`의 트리거는 `<button>`이라 해당 없음                                                           |
| 2   | `dialog/dialog-title.svelte`                                                                                                                                                                                                                                                               | `pr-8` 추가, `leading-none` 제거                                                                            | 두 줄짜리 한글 제목이 오른쪽 위 닫기 버튼과 겹치고 줄이 붙는다                                                                                                                                                                                                              |
| 2   | `sheet/sheet-title.svelte`                                                                                                                                                                                                                                                                 | `pr-8` 추가                                                                                                 | 같은 닫기 버튼이 같은 자리에 있다                                                                                                                                                                                                                                           |
| 3   | `input/input.svelte`, `textarea/textarea.svelte`, `select/select-trigger.svelte`, `checkbox/checkbox.svelte`, `input-group/input-group.svelte`, `command/command-input.svelte`, `calendar/calendar-{month,year}-select.svelte`, `range-calendar/range-calendar-{month,year}-select.svelte` | `border-input`(또는 `border-input/30`) → `border-input-border`                                              | 생성물의 `--input`은 바탕과 1.2:1이다. 입력 칸 테두리용 토큰 `--input-border`(3:1 이상)를 새로 두었다. `--input`은 입력 칸 바탕(`bg-input/30`)에 그대로 쓴다                                                                                                                |
| 3   | `switch/switch.svelte`                                                                                                                                                                                                                                                                     | `data-unchecked:bg-input` → `data-unchecked:bg-input-border`                                                | 밝은 화면에서 꺼진 스위치의 트랙과 손잡이가 바탕과 구분되지 않는다(1.2:1)                                                                                                                                                                                                   |
| 4   | `sidebar/sidebar-menu-button.svelte`, `sidebar/sidebar-menu-sub-button.svelte`                                                                                                                                                                                                             | `"data-active": isActive` → `isActive ? true : undefined`                                                   | 거짓일 때 속성 자체를 뺀다. 1.8.0의 `shadcn-svelte/tailwind.css`는 `data-active:` 변형을 `[data-active]:not([data-active="false"])`로 정의해 이미 거짓을 걸러 내지만(빌드된 CSS에서 확인), 앱 코드가 `data-[active]:`처럼 속성의 존재만 보는 선택자를 써도 틀리지 않게 한다 |
| 5   | `dialog/dialog-content.svelte`, `dialog/dialog-footer.svelte`, `sheet/sheet-content.svelte`                                                                                                                                                                                                | `Close` → `m.ui_close()`                                                                                    | 영어 고정 문구                                                                                                                                                                                                                                                              |
| 5   | `sidebar/sidebar-trigger.svelte`, `sidebar/sidebar-rail.svelte`                                                                                                                                                                                                                            | `Toggle Sidebar` → `m.ui_toggle_sidebar()` (`aria-label`, `title` 포함)                                     | 영어 고정 문구                                                                                                                                                                                                                                                              |
| 5   | `sidebar/sidebar.svelte`                                                                                                                                                                                                                                                                   | 모바일 서랍의 제목·설명 → `m.ui_sidebar_title()`, `m.ui_sidebar_description()`                              | 영어 고정 문구(화면 낭독기용)                                                                                                                                                                                                                                               |
| 5   | `command/command-dialog.svelte`                                                                                                                                                                                                                                                            | `title`, `description`의 기본값 → `m.ui_command_title()`, `m.ui_command_description()`                      | 영어 고정 문구(화면 낭독기용)                                                                                                                                                                                                                                               |
| 6   | `sonner/sonner.svelte`                                                                                                                                                                                                                                                                     | `mode-watcher`의 `mode.current` → `$lib/theme.svelte`의 `theme.resolved`                                    | 화면 밝기는 앱의 상태 모듈 하나가 정한다. 의존성도 하나 줄어든다                                                                                                                                                                                                            |
| —   | `calendar/calendar-caption.svelte`, `range-calendar/range-calendar-caption.svelte`                                                                                                                                                                                                         | `date.toDate(getLocalTimeZone())` → `date.toDate("UTC")`, 포매터에 `timeZone: "UTC"` (2026-11-10 19:10 KST) | 달력의 달은 달력 날짜라 시간대가 필요 없다. 기기의 시간대를 읽는 곳은 `src/lib/regional.ts` 하나로 모은다                                                                                                                                                                   |
| 7   | `sheet/sheet-overlay.svelte`, `dialog/dialog-overlay.svelte`, `alert-dialog/alert-dialog-overlay.svelte`                                                                                                                                                                                   | `bg-black/10 supports-backdrop-filter:backdrop-blur-xs` → `scrim` (2026-11-10 22:50 KST)                    | 색을 직접 적지 않는다. 뒤를 가리는 층의 어둡기와 흐림은 토큰 `--scrim`, `--scrim-blur`가 정하고, 기기의 "투명도 줄이기" 설정에서는 흐림이 꺼진다                                                                                                                            |
| 8   | `dialog/dialog-content.svelte`                                                                                                                                                                                                                                                             | `top-1/2` → `top-[calc((100dvh-var(--keyboard))/2)]` (2026-11-27 KST)                                       | iPhone Safari는 키보드가 올라와도 화면을 줄이지 않는다. 가운데의 대화 상자가 키보드에 가려 입력란이 보이지 않는다. 높이는 대화 상자마다 정한다(전체 화면인 것은 `top-0`으로 이것을 덮는다)                                                                                  |

점검 목록의 나머지 두 항목은 이 폴더 밖에서 처리했다.

- UI 기본 글자 0.9375rem: 컴포넌트는 `text-sm`을 그대로 쓰고, `src/styles/app.css`에서 `--text-sm`을 0.9375rem으로 정의했다.
- 토큰 대비 검사: `tests/tokens/contrast.test.ts`.

## 규칙

- 수정은 최소로 하고, 디자인 변경은 `src/styles/app.css`의 토큰으로 한다.
- 새 컴포넌트는 같은 CLI로 추가하고 이 문서의 명령과 표에 덧붙인다. 추가한 뒤에는 `package.json`과 `pnpm-lock.yaml`의 변화를 확인해 고정 판으로 되돌린다.
- 이 폴더와 `hooks/`, `utils.ts`는 Prettier 대상에서 뺐다(생성기의 서식을 유지해야 다시 생성한 파일과 비교할 수 있다). ESLint는 적용하되 서식·취향 규칙은 껐다(`eslint.config.js`의 `GENERATED_UI`).
- 앱 고유 컴포넌트는 `src/lib/features/<기능>/`에 둔다. 여기에 두지 않는다.
