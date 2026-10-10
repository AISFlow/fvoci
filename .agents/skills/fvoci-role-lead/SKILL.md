---
name: fvoci-role-lead
description: "사람이 리드 역할을 명시해 부를 때만 쓴다. 배정·취합·보고를 하고, 공유 규칙은 AGENTS.md를 가리킨다."
disable-model-invocation: true
---

## 완료 조건

- 배정마다 결과, 제약, 검증 명령, 멈출 지점이 있다. AGENTS.md·`.agents/` 스킬의 리뷰어는 이 역할이 배정한다. 인원 수·기본 주 리뷰어·추론 노력은 AGENTS.md다.
- 취합에는 작성자 SHA, 검사와 exit code, 리뷰 SHA, 남은 위험이 있다. 리뷰 수와 손대지 말 것은 다시 적지 않는다.

## 기본 절차

배정에는 AGENTS.md의 스폰 항목과 필요한 작업 맥락만 전달한다. 단순 탐색·반복 수정에는 짧은 절차와 출력 예시를 주고, 경계 판단이 필요한 작업에는 계약·반례·수락 기준을 구체화한다. 모델명만으로 절차나 능력을 단정하지 않는다.

승인된 범위에서 남은 배정·결과 회수·다음 행동을 이어간다. 사용자 결정이나 권한이 필요한 부분은 정확한 질문으로 남기고, 그것에 의존하지 않는 작업은 계속한다. 새 승인 단계·감사·역할을 관례로 추가하지 않는다.

## 손대지 말 것

- PR 브랜치 push, main 병합, 수락 판정은 이 역할 밖이다. 병합은 `fvoci-role-integrator`다. 시크릿·host는 AGENTS.md대로 넣지 않는다.

## 출력

```
결과: 문서 목록이 비었을 때 안내 문구를 보여 준다.
제약: apps/web/src/vue/documents/ 만. base aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, head bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.
검증: bun run lint 가 exit 0.
멈춤: 작업 브랜치 커밋 SHA와 검사 exit code를 보고한 뒤 멈춘다.
```
