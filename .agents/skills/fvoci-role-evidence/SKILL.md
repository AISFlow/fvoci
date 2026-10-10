---
name: fvoci-role-evidence
description: "사람이 증거 역할을 명시해 부를 때만 쓴다. 병합·리뷰·결정마다 추가만 하는 장부 항목을 남긴다."
disable-model-invocation: true
---

# 증거

## 완료 조건

- 이 역할은 장부다. 흐름을 쫓는 tracer가 아니다.
- 병합, 리뷰, 결정마다 항목을 하나 추가한다. 항목에는 KST 시각, 40자 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, 리뷰어별 판정, 게이트 checkout 로그의 tested merge SHA, PASS/FAIL/NOTRUN/MISSING이 있다.
- 고칠 때는 기존 항목을 바꾸지 않고 새 항목을 추가한다.
- 확인하지 않은 주장은 unverified로 적는다.
- 영환님 말이 필요한 결정은 영환님에게 1:1로 먼저 보낸다.
- 이전 SHA의 결과를 이 항목에 합치지 않는다. AGENTS.md 완료 조건.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

게이트 checkout 로그에서 tested merge SHA를 읽는다. 없는 값은 MISSING이다. 판정이 없으면 없음이라고 적는다.

## 손대지 말 것

- 코드를 고치지 않는다. 워크플로를 재실행하거나 취소하지 않는다.
- 장부 경로와 링크는 이 스킬에 적지 않는다.
- 시크릿·credential·접속 URL·host·봇 식별자는 항목에 남기지 않는다. AGENTS.md 손대지 말 것.

## 출력

```
시각: <KST>
종류: merge | review | decision
SHA: <40자>
parent: <40자>
patch-sha256: <64자>
patch-id: <git patch-id --stable 첫 필드>
판정:
- rust: ACCEPT | REQUEST_CHANGES | 없음
- ci-web: ACCEPT | REQUEST_CHANGES | 없음
- db: ACCEPT | REQUEST_CHANGES | 없음
tested-merge: <게이트 checkout 로그 SHA | MISSING>
결과: <게이트 또는 검사> PASS|FAIL|NOTRUN|MISSING
비고: <unverified | 정정 | 없음>
```

```
시각: 2026-10-10 12:00 KST
종류: review
SHA: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
parent: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
patch-id: dddddddddddddddddddddddddddddddddddddddd
판정:
- rust: ACCEPT
- ci-web: REQUEST_CHANGES
- db: 없음
tested-merge: MISSING
결과: bash scripts/test-ci-selection.sh NOTRUN
비고: 없음
```
