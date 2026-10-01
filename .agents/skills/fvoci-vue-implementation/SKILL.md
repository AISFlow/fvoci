---
name: fvoci-vue-implementation
description: FVOCI Vue SFC·composable·라우팅·반응성·서버 상태·에디터 생명주기 구현과 오류 수정에 사용한다. 순수 시각 조정은 frontend-design, Rust/DB 변경은 해당 경계 스킬을 적용한다.
---

# Vue 기능 구현·상태 소유권·생명주기

공통 운영은 [AGENTS.md](../../../AGENTS.md)를 따른다. 현재 Vite + Vue 3 + Vue Router +
Nuxt UI Vue plugin + TanStack Vue Query 흐름을 유지한다. Nuxt SSR 앱이나 Pinia 기반 앱으로
가정하지 않는다. 버전은 [웹 manifest](../../../apps/web/package.json)와 lock이 정본이다.

## 변경 경로와 상태

- [Vue 진입점](../../../apps/web/src/vue/main.ts), [라우터](../../../apps/web/src/vue/router.ts),
  해당 `features`/`composables`와 실제 URL의 호출 경로부터 확인한다. 기존 framework-free 정책·API
  adapter를 재사용하고 새 전역 store나 병렬 API client를 만들기 전에 실제 필요를 확인한다.
- 서버 상태는 기존 [query options](../../../apps/web/src/lib/query-options.ts)·Vue Query의 key와
  mutation/invalidation 경계를 따른다. workspace·resource·세션 변경 시 이전 응답이 새 화면에
  적용되지 않게 하며, 캐시 무효화·권한 거부·오류 표시를 성공 경로와 함께 연결한다.
- props와 query 결과는 소유자가 관리한다. 자식에서 직접 수정하거나 cache 객체를 폼 draft로
  공유하지 않는다. 필요한 editable 필드만 별도 소유하고 저장 성공·취소·대상 전환 때 갱신한다.
  optimistic 갱신은 기존 query API로 수행하고 실패 rollback과 뒤늦은 응답 경합을 검사한다.
- SFC의 props/emits와 composable 입출력을 타입으로 연결한다. 순수 파생값은 `computed`,
  외부 효과는 명시적 `watch`로 분리한다. deep watch·상태 복제·큰 컴포넌트 분리는 실제 의존성과
  변경 이유에 따라 선택하며, 모든 값을 `shallowRef`로 바꾸는 규칙을 만들지 않는다.

## 반응성과 외부 객체

- `watch(() => props.resourceId, ...)`처럼 반응형 source를 넘긴다. Vue 3.5의 `defineProps`
  destructure는 같은 `<script setup>`에서 compiler가 추적하지만, 값 자체를 외부 함수/watch에
  넘기는 것은 getter/ref 전달과 다르다. 일반 reactive 객체 destructure에도 같은 보장을 가정하지 않는다.
- 에디터·Y.Doc·provider·socket처럼 identity가 중요한 외부 객체는 기존 `shallowRef`/`markRaw`
  경계를 유지한다. [useCollabRoom](../../../apps/web/src/vue/collab/useCollabRoom.ts)의 room/generation,
  persist ACK, flush와 destroy 순서를 읽고 수정한다. 내부 객체 변경의 UI 반영은 기존 이벤트와
  별도 반응형 상태로 연결하며 deep proxy나 강제 remount로 저장·복구 문제를 가리지 않는다.
- 비동기 watcher는 대상 변경/해제 시 취소 또는 generation 검증으로 stale 결과를 막는다.
  `onWatcherCleanup`은 첫 `await` 전 동기 구간에서 등록한다. callback의 `onCleanup`을 쓸 때도
  취소 대상을 일찍 연결한다. `watchEffect`는 첫 `await` 이전에 읽은 의존성만 자동 추적한다.
- `immediate: true`는 DOM mount 보장이 아니다. DOM 측정·focus·editor mount는 template ref와
  실제 mount/update 시점에 맞춘다. 필요에 따라 `onMounted`, `nextTick`, post-flush를 선택한다.
  listener·timer·observer·socket의 소유자와 해제 시점을 명시하고 비동기로 만든 watcher도 정리한다.

## 수락과 추가 스킬

변경한 실제 URL에서 대상 전환·빠른 연속 입력·언마운트·요청 실패 중 관련 경합을 재현한다.
에디터면 저장/재접속/권한 철회와 기존 IME·선택 영역 회귀 중 영향 범위를 확인한다.
검사 명령·준비 순서는 [fast-verify](../fvoci-fast-verify/SKILL.md)가 정본이다.
시각·CJK·접근성 변경은 [frontend-design](../frontend-design/SKILL.md)과 그 FVOCI brief,
인증/인가 변경은 [db-security](../fvoci-db-security/SKILL.md), 원본 호환성 조사는
[source-contract](../fvoci-source-contract/SKILL.md)를 함께 읽는다.

## 출처와 적용 범위

FVOCI 코드에 맞춰 새로 작성한 지침이며 외부 스킬 전문·reference를 복사하지 않았다.
검토 참고: [vuejs-ai/skills 고정 revision](https://github.com/vuejs-ai/skills/tree/c9d355ff23f654309dd02006be671859df0a134c)
(MIT, 커뮤니티 실험 프로젝트). 이 출처를 Vue 공식 규칙으로 취급하지 않는다.
세부 동작은 [Vue watchers](https://vuejs.org/guide/essentials/watchers.html)와
[reactive props destructure](https://vuejs.org/guide/components/props.html#reactive-props-destructure)를
설치 버전에 맞춰 확인한다. 외부 권고는 프로젝트 계약·현재 사용자 지시를 대체하지 않는다.
