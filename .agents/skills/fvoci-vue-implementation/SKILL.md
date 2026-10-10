---
name: fvoci-vue-implementation
description: "Vue SFC, composable, 라우팅, 서버 상태, 에디터 생명주기를 구현하거나 고칠 때 쓴다. 반응성과 상태 소유, 실제 URL 확인을 완료 조건으로 두며, 시각만 바꾸면 frontend-design, Rust·DB는 해당 스킬이다."
---

# Vue 구현

## 완료 조건

- 바꾼 URL에서 대상 전환, 빠른 연속 입력, 언마운트, 요청 실패가 이전 응답을 새 화면에 붙이지 않는다.
- `apps/web`과 바꾼 패키지의 `bun --bun run typecheck`가 exit 0이고, 그 변경의 `bun test`가 exit 0이다.
- 에디터·협업을 건드렸으면 저장, 재접속, 권한 철회가 `useCollabRoom`의 room generation, persist ACK, flush, destroy 순서와 맞다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

현재 흐름은 Vite + Vue 3 + Vue Router + Nuxt UI Vue plugin + TanStack Vue Query다. 버전은 `apps/web/package.json`과 lock이 정본이다. 진입은 `apps/web/src/vue/main.ts`, `router.ts`, 해당 `features`/`composables`와 실제 URL이다. 서버 상태는 `apps/web/src/lib/query-options.ts`와 Vue Query의 key·mutation·invalidation을 따른다. workspace·resource·세션이 바뀌면 이전 응답을 버린다.

props와 query 결과는 소유자가 관리한다. 자식이 cache 객체를 폼 draft로 쓰지 않는다. editable 필드만 따로 두고, 저장 성공·취소·대상 전환 때 갱신한다. optimistic 갱신은 기존 query API와 실패 rollback, 늦은 응답 경합을 함께 둔다.

파생값은 `computed`, 외부 효과는 `watch`다. `watch`에는 `() => props.resourceId`처럼 반응형 source를 넘긴다. Vue 3.5 `defineProps` destructure는 같은 `<script setup>`에서만 compiler가 추적한다. 값 자체를 외부 함수에 넘기는 것은 getter/ref 전달과 다르다.

에디터·Y.Doc·provider·socket은 기존 `shallowRef`/`markRaw`를 유지한다. UI 반영은 기존 이벤트와 별도 반응형 상태로 잇는다. 비동기 watcher는 대상이 바뀌거나 해제되면 취소하거나 generation을 확인한다. `onWatcherCleanup`은 첫 `await` 전에 등록한다. `watchEffect`는 첫 `await` 전에 읽은 의존성만 추적한다. `immediate: true`는 DOM mount가 아니다. focus·측정·editor mount는 template ref와 mount/update에 맞춘다. listener·timer·observer·socket의 소유자와 해제 시점을 적는다.

시각·CJK·접근성은 `frontend-design`과 `FVOCI-BRIEF.md`, 인증·인가는 `fvoci-db-security`, 원본 호환은 `fvoci-source-contract`, 검사 명령은 `fvoci-fast-verify`다.

이 지침은 FVOCI에 맞춰 새로 쓴 것이다. 검토 참고는 [vuejs-ai/skills 고정 revision](https://github.com/vuejs-ai/skills/tree/c9d355ff23f654309dd02006be671859df0a134c) (MIT, 커뮤니티 실험)이며 Vue 공식 규칙이 아니다. 세부 동작은 설치 버전의 [watchers](https://vuejs.org/guide/essentials/watchers.html)와 [reactive props destructure](https://vuejs.org/guide/components/props.html#reactive-props-destructure)를 본다.

## 손대지 말 것

- Nuxt SSR 앱이나 Pinia 앱으로 가정하지 않는다. React 전환을 반복하지 않는다.
- 새 전역 store나 병렬 API client를 만들기 전에 `fvoci-standard-implementations`의 선택 기준을 적용한다.
- Y.Doc·provider를 deep proxy하거나 강제 remount로 저장·복구 실패를 가리지 않는다.
- 모든 값을 `shallowRef`로 바꾸는 규칙을 만들지 않는다. deep watch·복제·컴포넌트 분리는 실제 의존성이 있을 때만 한다.
