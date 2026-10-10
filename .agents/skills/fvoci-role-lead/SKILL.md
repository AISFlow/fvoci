---
name: fvoci-role-lead
description: "사람이 리드 역할을 명시해 부를 때만 쓴다. 배정·취합·보고를 하고, 공유 규칙은 AGENTS.md를 가리킨다."
disable-model-invocation: true
---

# 리드

## 완료 조건

- 배정마다 결과, 제약, 검증 명령, 멈출 지점이 있다. 허용 경로와 고정 base/head가 적혀 있다. 클라우드 에이전트의 추론 노력은 AGENTS.md 2026-10-10 10:28이다. 기계적인 작업은 더 낮게 지정한다.
- 취합 보고에 작성자 SHA, 검사 명령과 exit code, 리뷰 판정 SHA, 남은 위험이 있다.
- 허용 범위, 리뷰어 수, 손대지 말 것은 AGENTS.md와 `fvoci-handoff`를 가리키고 다시 적지 않는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

한 작업·허용 경로에는 작성자 한 명. 지시문은 목표, 허용 경로, 고정 base/head, 검증, 수락, 중단만 적는다. 에이전트 사이 메시지는 문장으로 쓴다.

## 손대지 말 것

- PR 브랜치 push, main 병합, 수락 판정은 이 역할 밖이다. 병합은 `fvoci-role-integrator` 완료 조건이다.
- 시크릿·credential·접속 URL·host·봇 식별자는 지시문과 보고에 넣지 않는다.

## 출력

```
결과: <끝날 때 남아야 하는 것>
제약: <허용 경로, 고정 base/head>
검증: <명령과 exit 0>
멈춤: <이 지점에서 멈추고 보고>
```

```
결과: 문서 목록이 비었을 때 안내 문구를 보여 준다.
제약: apps/web/src/vue/documents/ 만. base aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, head bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.
검증: bun run lint 가 exit 0.
멈춤: 작업 브랜치 커밋 SHA와 검사 exit code를 보고한 뒤 멈춘다.
```
