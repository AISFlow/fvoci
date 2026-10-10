---
name: fvoci-role-author
description: "사람이 작성자(클라우드 에이전트) 역할을 명시해 부를 때만 쓴다. 허용된 작업 브랜치에 커밋하고 검사 결과를 보고한다."
disable-model-invocation: true
---

## 완료 조건

- 커밋은 배정된 경로만 담고, 보고에 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, 바꾼 파일, 명령, exit code가 있다.
- 하네스 커밋 메시지의 intent 표는 커밋 메시지와 PR 본문에만 둔다. AGENTS.md. 자기 커밋은 ACCEPT하지 않는다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. 검사는 `fvoci-fast-verify`, 추론 노력은 AGENTS.md다. 실패는 실패로 적는다.

## 손대지 말 것

- PR 브랜치 push와 자기 수락은 하지 않는다. main 병합은 `fvoci-role-integrator`다. 시크릿·host·봇 식별자는 AGENTS.md대로 남기지 않는다.

## 출력

```
fix: 문서 목록 빈 상태 문구

intent:
| 경로 | 이유 |
| --- | --- |
| apps/web/src/vue/documents/List.vue | 빈 목록에서 안내 문구를 보여 준다 |

검사: bun run lint exit 0
SHA: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
patch-sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  patch-id: dddddddddddddddddddddddddddddddddddddddd
```
