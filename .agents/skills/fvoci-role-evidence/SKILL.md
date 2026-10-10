---
name: fvoci-role-evidence
description: "사람이 증거 역할을 명시해 부를 때만 쓴다. 병합·리뷰·결정마다 추가만 하는 장부 항목을 남긴다."
disable-model-invocation: true
---

## 완료 조건

- 이 역할은 장부다. tracer가 아니다. 병합·리뷰·결정마다 항목을 하나 추가하고, 고칠 때는 새 항목만 추가한다.
- 항목에는 KST 시각, 40자 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, 리뷰어별 판정, 게이트 checkout 로그의 tested merge SHA, PASS/FAIL/NOTRUN/MISSING이 있다.
- 확인하지 않은 주장은 unverified다. 메인테이너 말이 필요한 결정은 메인테이너에게 1:1로 먼저 보낸다. 이전 SHA를 합치지 않는다. AGENTS.md.

## 기본 절차

tested merge는 게이트 checkout 로그에서 읽고, 없으면 MISSING이다.

## 손대지 말 것

- 코드를 고치지 않고 워크플로를 재실행하지 않는다. 장부 경로·링크는 이 스킬에 적지 않는다. 시크릿·host·봇 식별자는 AGENTS.md대로 남기지 않는다.

## 출력

```
시각: 2026-10-10 12:00 KST
종류: review
SHA: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  parent: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  patch-id: dddddddddddddddddddddddddddddddddddddddd
판정: rust ACCEPT, ci-web REQUEST_CHANGES, db 없음
tested-merge: MISSING
결과: bash scripts/test-ci-selection.sh NOTRUN
비고: 없음
```
