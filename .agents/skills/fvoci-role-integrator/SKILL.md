---
name: fvoci-role-integrator
description: "사람이 통합 역할을 명시해 부를 때만 쓴다. main PR을 병합하고, #347 fast-forward는 하지 않는다."
disable-model-invocation: true
---

# 통합

## 완료 조건

- 이 역할은 main PR을 병합한다. #347 fast-forward는 하지 않는다.
- 작은 커밋은 ACCEPT 1명이다. workflow, xtask, AGENTS.md, `.agents/`, 게이트, ruleset, `scripts/ci_selection.py`는 ACCEPT 2/2다. `docs/rewrite.md`는 1명이다. AGENTS.md 허용 범위와 같다.
- 최종 head에서 게이트 5개(`rust-ci-gate`, `web-ci-gate`, `install-ci-gate`, `documents-ci-gate`, `collab-engine-ci-gate`)가 PASS다. FAIL, NOTRUN, MISSING은 거절한다. CANCELLED·SKIP은 AGENTS.md대로 PASS가 아니다.
- 병합 SHA를 방에 먼저 게시한 뒤에 병합한다.
- 게이트 checkout 로그의 tested merge 첫 부모가 현재 main SHA와 같다. API `head_sha`로 이 값을 대체하지 않는다. `filter=latest`, 다른 check run, squash·rebase 금지의 나머지는 `fvoci-handoff` 완료 조건이다.
- 머지 큐로 바꾼 뒤에는 큐 head를 먼저 게시하고, `merge_group` checkout SHA가 그 main 커밋 SHA와 같다.
- ruleset 변경은 변경 전 JSON과 변경 후 JSON의 sha256이 있다.
- Turso는 나머지 게이트 5개가 PASS한 뒤 최종 head 40자 SHA로 한 번만 dispatch한다. run의 head_sha가 그 SHA와 다르면 MISSING이다.
- 재실행은 전체 rerun뿐이다. `gh run rerun --failed`는 AGENTS.md 손대지 말 것 그대로 범위 밖이다.
- 브랜치 삭제는 각 브랜치가 적은 커밋을 여전히 가리키는지, 그리고 main에 포함됐는지를 다시 확인하고, 삭제한 목록을 남긴다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

병합 직전에 head, parent, 게이트 checkout 로그를 다시 읽는다. 알림은 head와 tested merge를 다른 줄에 적는다.

## 손대지 말 것

- auto-merge, 태그, 릴리스, 배포, 1.0.0 병합, 영환님이 직접 병합하는 PR은 이 역할이 하지 않는다.
- 커밋 내용을 고치지 않는다. force push와 `reset --hard`는 허용 조건이 없다. AGENTS.md 손대지 말 것.

## 출력

```
통합: 병합 | 보류
head: <40자>
parent: <40자>
tested-merge: <게이트 checkout 로그 SHA>
첫-부모: <그 로그의 첫 부모>
gates: <5개 PASS|FAIL|NOTRUN|MISSING>
큐-head: <40자 | 큐 전환 전>
ruleset-sha256: <전 64> <후 64 | 없음>
삭제: <브랜치=SHA 목록 | 없음>
```

```
통합: 병합
head: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
tested-merge: cccccccccccccccccccccccccccccccccccccccc
첫-부모: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
gates: rust-ci-gate PASS, web-ci-gate PASS, install-ci-gate PASS, documents-ci-gate PASS, collab-engine-ci-gate PASS
큐-head: 큐 전환 전
ruleset-sha256: 없음
삭제: 없음
```
