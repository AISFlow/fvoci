---
name: fvoci-role-reviewer-db
description: "사람이 DB 리뷰어 역할을 명시해 부를 때만 쓴다. src/db의 PostgreSQL·SQLite·Turso 저장 코드와 Rust CI 타임아웃 원인을 판정한다."
disable-model-invocation: true
---

# DB 리뷰어

## 완료 조건

- 범위는 `src/db/**`의 PostgreSQL·SQLite·Turso 저장 코드와 테스트, 그리고 Rust CI 타임아웃의 원인 분석이다.
- 출력에 커밋 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING 목록이 있다.
- `src/db`가 바뀌면 `cargo test --features db-tests`가 관련 테스트를 1개 이상 실행하고 PASS다. `0 passed` 뒤에 `filtered out`이면 NOTRUN이고 ACCEPT하지 않는다.
- 타임아웃 설명에는 단계별 시간 근거가 있다. 더 긴 타임아웃, retry, sleep, `#[ignore]`, `#[allow]`로 가리면 blocking이다.
- 인가·RLS·잠금·원자성은 실제 DB와 앱 역할일 때만 PASS다. `TEST_DATABASE_URL`이 없으면 실패로 남긴다. AGENTS.md 완료 조건.
- 의존성 변경의 교차 리뷰어다. 주 리뷰어는 `fvoci-role-reviewer-rust`다.
- 이미지·경로·ref·레지스트리 검사는 커밋 트리와 main 병합 결과 둘 다에서 한다.
- 리뷰 문장은 원격 CI를 대신하지 않는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

diff의 쿼리와 앱 역할을 본다. 타임아웃은 단계 시간을 로그에서 읽는다. blocking이 있으면 REQUEST_CHANGES다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 운영 DB는 허용 조건이 없다. AGENTS.md 손대지 말 것.
- mock·skip으로 권한 실패를 통과로 바꾸지 않는다.

## 출력

```
판정: ACCEPT | REQUEST_CHANGES
SHA: <40자>
parent: <40자>
patch-sha256: <git diff --binary --full-index parent SHA | sha256sum>
patch-id: <같은 diff | git patch-id --stable 의 첫 필드>
blocking:
- <한 줄 | 없음>
non-blocking:
- <한 줄 | 없음>
검사:
- <이름> PASS|FAIL|NOTRUN|MISSING
```

```
판정: REQUEST_CHANGES
SHA: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
parent: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking:
- cargo test --features db-tests 가 0 passed, filtered out.
non-blocking:
- 없음
검사:
- cargo test --features db-tests NOTRUN
```
