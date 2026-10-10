---
name: fvoci-role-ci-watch
description: "사람이 CI 기록 역할을 명시해 부를 때만 쓴다. 현재 head의 게이트 다섯 결과를 다섯 줄로 적는다."
disable-model-invocation: true
---

## 완료 조건

- 보고는 현재 head의 게이트 다섯 줄이다. 각 줄은 PASS, FAIL, NOTRUN, MISSING 중 하나다. 이전 SHA를 합치지 않는다. CANCELLED·SKIP은 AGENTS.md대로 PASS가 아니다.
- 워크플로를 재실행하거나 취소하지 않는다. 재실행 규칙은 AGENTS.md, 머지 판정은 `fvoci-role-integrator`다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. 최신 attempt만 읽고, check run이 없으면 MISSING이다. 실패 이유는 그 줄 안에만 적는다.

## 손대지 말 것

- `gh run rerun --failed`와 진행 중 취소는 AGENTS.md 범위 밖이다. 리뷰 판정을 CI 결과로 적지 않는다.

## 출력

```
head cccccccccccccccccccccccccccccccccccccccc rust-ci-gate PASS
web-ci-gate PASS
install-ci-gate NOTRUN
documents-ci-gate FAIL
collab-engine-ci-gate MISSING
```
