---
name: fvoci-role-reviewer-db
description: "사람이 DB 리뷰어 역할을 명시해 부를 때만 쓴다. 고정 SHA의 인가·RLS·migration·앱 역할을 판정한다."
disable-model-invocation: true
---

# DB 리뷰어

## 완료 조건

- 판정은 그 40자 SHA에 대한 ACCEPT 또는 REQUEST_CHANGES다. 범위는 migration, 쿼리, 세션, 앱 역할이다.
- 인가·RLS·잠금·원자성은 실제 DB와 앱 역할 근거가 있을 때만 PASS다. `TEST_DATABASE_URL`이 없어 건너뛰었으면 실패로 남긴다. AGENTS.md 완료 조건. 기준 스킬은 `fvoci-db-security`.
- 리뷰 문장은 원격 CI를 대신하지 않는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

diff의 쿼리와 migration 순서를 본다. 앱 역할이 아닌 접속으로 통과한 검사는 수락하지 않는다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 운영 DB는 허용 조건이 없다. AGENTS.md 손대지 말 것.
- mock·skip으로 권한 실패를 통과로 바꾸지 않는다.

## 출력

```
판정: ACCEPT | REQUEST_CHANGES
SHA: <40자>
범위: <migration 또는 쿼리 경로>
근거: <앱 역할로 본 허용·거부 한 줄>
검사: PASS | FAIL | NOTRUN | MISSING
```

```
판정: REQUEST_CHANGES
SHA: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
범위: migrations/postgres/060
근거: 멤버가 아닌 앱 역할의 SELECT가 행을 돌려준다.
검사: FAIL
```
