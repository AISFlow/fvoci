---
name: fvoci-role-reviewer-ci-web
description: "사람이 CI·웹 리뷰어 역할을 명시해 부를 때만 쓴다. 워크플로·게이트·웹 변경을 커밋 트리와 main 병합 결과에서 판정한다."
disable-model-invocation: true
---

## 완료 조건

- 범위는 workflow, `scripts/ci_selection.py`, 게이트, `apps/web`, `packages/`다. 인원 수는 이 스킬에 없고 AGENTS.md의 리뷰 수 한 규칙이다.
- 출력에 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING이 있다.
- workflow 또는 `scripts/ci_selection.py`가 바뀌면 `python3 scripts/ci_selection.py verify-workflows`와 `bash scripts/test-ci-selection.sh`가 커밋 트리와 main 병합 결과 둘 다 exit 0이다. 필수다. NOTRUN이면 ACCEPT하지 않는다.
- validator, allowlist, 게이트 로직이 바뀌면 예전 반례를 재현하는 mutation이 테스트를 FAIL 시킨다. 하네스 이전 예외는 `run:`와 레지스트리 데이터뿐이고, `on:`, `paths:`, permissions, 게이트 로직 변경은 blocking이다.
- 양쪽 트리 검사, `JEST_WORKER_ID`, 웹 테스트 명령은 AGENTS.md다. 어기면 blocking이다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. workflow와 `scripts/ci_selection.py`, 웹 진입점을 먼저 본다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 워크플로를 재실행하거나 취소하지 않는다. Rust는 `fvoci-role-reviewer-rust`, DB는 `fvoci-role-reviewer-db`다.

## 출력

```
판정: ACCEPT
SHA: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking: 없음
검사: verify-workflows 커밋 트리 PASS, main 병합 결과 PASS. bash scripts/test-ci-selection.sh 커밋 트리 PASS, main 병합 결과 PASS.
```
