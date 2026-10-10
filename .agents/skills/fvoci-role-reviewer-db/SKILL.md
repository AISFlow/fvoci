---
name: fvoci-role-reviewer-db
description: "사람이 DB 리뷰어 역할을 명시해 부를 때만 쓴다. src/db의 PostgreSQL·SQLite·Turso 저장 코드와 Rust CI 타임아웃 원인을 판정한다."
disable-model-invocation: true
---

## 완료 조건

- 범위는 `src/db/**`의 PostgreSQL·SQLite·Turso 저장 코드와 테스트, 그리고 Rust CI 타임아웃 원인 분석이다. 인가·의존성 주 리뷰어·양쪽 트리 검사는 AGENTS.md다.
- 출력에 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING이 있다.
- `src/db`가 바뀌면 `cargo test --features db-tests`가 관련 테스트를 1개 이상 실행하고 PASS다. `0 passed` 뒤에 `filtered out`이면 NOTRUN이고 ACCEPT하지 않는다.
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
