# 기능 폴더

화면은 기능별로 `features/<기능>/` 아래에 둔다(예: `board`, `card`, `sidebar`, `views/kanban`).

규칙(ESLint `fvoci/feature-boundaries`, `tools/lint/layers.ts`):

- 기능 폴더는 다른 기능 폴더에서 가져오지 않는다. `$lib/features/다른기능/…`도, `../다른기능/…`도 안 된다.
- 두 기능이 같이 쓰는 것은 아래 층으로 내린다: 계산은 `lib/logic`, 상태는 `lib/state`, 서버와의 통신은 `lib/api`, 화면 부품은 `lib/ui`(shadcn-svelte가 만든 것)와 `lib/components`(직접 쓴 것. 그 폴더의 README). 기능은 서버를 직접 부르지 않고 `lib/state`로만 읽고 쓴다(`fvoci/no-direct-server`).
- 아래 층(`logic`, `state`, `api`, `ui`, `components` 등 `features`가 아닌 모든 폴더)은 기능 폴더에서 가져오지 않는다.
- 기능을 조립하는 곳은 `src/app`이다. 거기서는 어느 기능이든 가져올 수 있다.

`features/` 바로 아래의 첫 폴더 이름이 기능의 이름이다. `features/views/kanban`과 `features/views/table`은 같은 기능 `views`에 속한다.

기능이 다른 기능의 부품을 써야 할 때는 둘 가운데 하나다. 여러 기능이 늘 같이 쓰는 부품이면 아래 층으로 내린다(속성 값 부품이 그래서 `lib/components/properties/`에 있다). 한 화면이 다른 화면을 품는 관계이면(보드 페이지가 뷰를, 뷰가 카드 상세를 연다) `src/app`에서 조립하면서 스니펫이나 인자로 넘긴다. 규칙을 풀지 않는다.

층 전체의 그림은 `src/lib/README.md`에 있다.
