---
name: fvoci-role-reviewer-ci-web
description: "사람이 CI·웹 리뷰어 역할을 명시해 부를 때만 쓴다. 고정 SHA의 워크플로·게이트·웹 변경을 판정한다."
disable-model-invocation: true
---

# CI·웹 리뷰어

## 완료 조건

- 판정은 그 40자 SHA에 대한 ACCEPT 또는 REQUEST_CHANGES다. 워크플로·게이트·`apps/web`·`packages/`만 이 역할이 본다.
- 게이트에 `paths:`가 없는지는 AGENTS.md 손대지 말 것과 대조한다. `bash scripts/test-ci-selection.sh`를 돌렸으면 exit code를 적고, 아니면 NOTRUN이다.
- 리뷰 문장은 원격 CI를 대신하지 않는다. 이 경로는 리뷰어 2명이다. AGENTS.md 허용 범위.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

diff에서 workflow, `scripts/ci_selection.py`, 웹 진입점을 먼저 본다. REQUEST_CHANGES에는 최소 수정안 한 가지를 적는다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 워크플로를 재실행하거나 취소하지 않는다.
- Rust 서버 계약과 DB 역할은 각 리뷰어 스킬에 맡긴다.

## 출력

```
판정: ACCEPT | REQUEST_CHANGES
SHA: <40자>
범위: <본 경로>
근거: <게이트 또는 웹 계약 한 줄>
검사: <명령 exit code | NOTRUN>
```

```
판정: ACCEPT
SHA: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
범위: .github/workflows/web.yml
근거: 게이트 파일에 paths 필터가 없다.
검사: bash scripts/test-ci-selection.sh exit 0
```
