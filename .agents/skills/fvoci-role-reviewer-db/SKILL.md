---
name: fvoci-role-reviewer-db
description: "사람이 DB 리뷰어 역할을 명시해 부를 때만 쓴다. PostgreSQL·SQLite·Turso 저장·migration과 경로에 관계없는 인증·인가·세션·앱 역할 계약, 관련 Rust CI 실패를 판정한다."
disable-model-invocation: true
---

## 완료 조건

- 범위는 `src/db/**`, `migrations/**`의 PostgreSQL·SQLite·Turso 저장 코드·스키마·테스트와 경로에 관계없는 인증·인가·세션·앱 역할 계약이다. REST·WS·SSE와 UI 경계는 해당 Rust·CI/웹 리뷰어와 함께 대조한다. 관련 Rust CI 타임아웃 원인도 분석한다. 배정·리뷰 수·양쪽 트리 검사는 AGENTS.md를 따른다.
- 출력에 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING이 있다.
- 저장·migration·인가 계약이 바뀌면 `fvoci-db-security`와 `fvoci-fast-verify`에 따라 대상 backend와 앱 역할의 관련 검사를 선택해 실행한다. `cargo test --locked --offline --features db-tests`에는 해당 target과 필터를 명시한다. 실행한 관련 테스트가 0개면 NOTRUN이고 ACCEPT하지 않는다. 문서만 바뀌면 실제 DB 전체 검사를 추가하지 않는다.
- 타임아웃 설명에는 단계별 시간 근거가 있다. 더 긴 타임아웃, retry, sleep, `#[ignore]`, `#[allow]`로 가리면 blocking이다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. 타임아웃은 단계 시간을 로그에서 읽는다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 운영 DB와 mock·skip 통과는 AGENTS.md에 허용 조건이 없다.

## 출력

```
판정: REQUEST_CHANGES
SHA: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  parent: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking: cargo test --features db-tests 가 0 passed, filtered out.
검사: cargo test --features db-tests NOTRUN
```
