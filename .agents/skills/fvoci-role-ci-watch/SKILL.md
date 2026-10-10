---
name: fvoci-role-ci-watch
description: "사람이 CI 기록 역할을 명시해 부를 때만 쓴다. 현재 head의 게이트 다섯 결과를 다섯 줄로 적는다."
disable-model-invocation: true
---

# CI 기록

## 완료 조건

- 보고는 현재 head만 다룬다. 출력은 게이트 다섯 줄이고, 각 줄은 PASS, FAIL, NOTRUN, MISSING 중 하나다.
- 이전 SHA의 결과를 이 head에 합치지 않는다. CANCELLED·SKIP은 PASS가 아니다. AGENTS.md 완료 조건.
- 워크플로를 재실행하거나 취소하지 않는다. 역할 경계는 `fvoci-handoff`의 CI 기록과 같다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

그 head의 최신 attempt만 읽는다. check run이 없으면 MISSING이다. 최초 실패 이유는 그 게이트 줄 안에만 적는다.

## 손대지 말 것

- `gh run rerun --failed`와 진행 중 취소는 AGENTS.md 손대지 말 것 그대로 범위 밖이다.
- 리뷰 판정을 CI 결과로 적지 않는다. 머지 판정은 `fvoci-role-integrator` 완료 조건이다.

## 출력

다섯 줄만 낸다. 첫 줄에 head SHA를 적는다.

```
head <40자> rust-ci-gate PASS|FAIL|NOTRUN|MISSING
web-ci-gate PASS|FAIL|NOTRUN|MISSING
install-ci-gate PASS|FAIL|NOTRUN|MISSING
documents-ci-gate PASS|FAIL|NOTRUN|MISSING
collab-engine-ci-gate PASS|FAIL|NOTRUN|MISSING
```

```
head cccccccccccccccccccccccccccccccccccccccc rust-ci-gate PASS
web-ci-gate PASS
install-ci-gate NOTRUN
documents-ci-gate FAIL
collab-engine-ci-gate MISSING
```
