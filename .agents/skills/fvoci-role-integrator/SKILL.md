---
name: fvoci-role-integrator
description: "사람이 통합 역할을 명시해 부를 때만 쓴다. main PR을 병합하고, #347 fast-forward는 하지 않는다."
disable-model-invocation: true
---

## 완료 조건

- 이 역할은 main PR을 병합한다. #347 fast-forward는 하지 않는다.
- 리뷰 수·게이트·재실행·머지 큐는 AGENTS.md다. 병합 순서, tested merge, Turso, 브랜치 삭제, ruleset은 `fvoci-handoff` 완료 조건이다.
- workflow·xtask·게이트의 0.x 병합은 2명 ACCEPT와 필수 CI PASS 뒤에, 병합 SHA와 규칙 요약을 게시한 다음 한다. AGENTS.md와 `.agents/` 스킬 커밋은 리뷰어 ACCEPT와 CI에 더해, 메인테이너가 그 40자 SHA를 명시한 뒤에만 병합한다(2026-10-10 11:08 KST).
- 알림은 head와 tested merge를 다른 줄에 적는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. 병합 직전에 head, parent, 게이트 checkout 로그를 다시 읽는다.

## 손대지 말 것

- auto-merge, 태그, 릴리스, 배포, 1.0.0 병합, 메인테이너가 직접 병합하는 PR은 이 역할이 하지 않는다. 허용 조건은 AGENTS.md다.
- 커밋 내용을 고치지 않는다. force push와 `reset --hard`는 AGENTS.md에 허용 조건이 없다.

## 출력

```
통합: 병합
head: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
tested-merge: cccccccccccccccccccccccccccccccccccccccc
첫-부모: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
gates: rust-ci-gate PASS, web-ci-gate PASS, install-ci-gate PASS, documents-ci-gate PASS, collab-engine-ci-gate PASS
큐-head: 큐 전환 전
ruleset-sha256: 없음
삭제: 없음
```
