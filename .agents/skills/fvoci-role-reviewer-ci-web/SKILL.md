---
name: fvoci-role-reviewer-ci-web
description: "사람이 CI·웹 리뷰어 역할을 명시해 부를 때만 쓴다. 워크플로·게이트·웹 변경을 커밋 트리와 main 병합 결과에서 판정한다."
disable-model-invocation: true
---

# CI·웹 리뷰어

## 완료 조건

- 범위는 workflow, `scripts/ci_selection.py`, 게이트, `apps/web`, `packages/`다. 리뷰어 수는 AGENTS.md 허용 범위다.
- 출력에 커밋 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING 목록이 있다.
- workflow 또는 `scripts/ci_selection.py`가 바뀌면 `python3 scripts/ci_selection.py verify-workflows`와 `bash scripts/test-ci-selection.sh`가 커밋 트리와 main 병합 결과 둘 다 exit 0이다. 이 검사는 필수다. NOTRUN이면 ACCEPT하지 않는다.
- validator, allowlist, 게이트 로직이 바뀌면 예전 반례를 재현하는 mutation이 테스트를 FAIL 시켜야 한다.
- 하네스 이전의 예외는 `run:` 줄과 레지스트리 데이터뿐이다. `on:`, `paths:`, permissions, 게이트 로직 변경은 blocking이다.
- 이미지·경로·ref·레지스트리 검사는 커밋 트리와 main 병합 결과 둘 다에서 한다.
- Playwright 자식 env에 `JEST_WORKER_ID`가 남아 있으면 blocking이다. 규칙은 AGENTS.md의 `process.execPath` 줄이다.
- 리뷰 문장은 원격 CI를 대신하지 않는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

diff에서 workflow, `scripts/ci_selection.py`, 웹 진입점을 먼저 본다. blocking이 있으면 REQUEST_CHANGES다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 워크플로를 재실행하거나 취소하지 않는다.
- Rust 서버 계약은 `fvoci-role-reviewer-rust`, DB 저장은 `fvoci-role-reviewer-db`다.

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
판정: ACCEPT
SHA: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking:
- 없음
non-blocking:
- 없음
검사:
- verify-workflows 커밋 트리 PASS
- verify-workflows main 병합 결과 PASS
- bash scripts/test-ci-selection.sh 커밋 트리 PASS
- bash scripts/test-ci-selection.sh main 병합 결과 PASS
```
