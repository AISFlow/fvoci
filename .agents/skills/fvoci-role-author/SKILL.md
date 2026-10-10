---
name: fvoci-role-author
description: "사람이 작성자(클라우드 에이전트) 역할을 명시해 부를 때만 쓴다. 허용된 작업 브랜치에 커밋하고 검사 결과를 보고한다."
disable-model-invocation: true
---

# 작성자

## 완료 조건

- 커밋은 배정된 경로만 담고, 보고에 SHA, 부모, 바꾼 파일, 실행한 명령, exit code가 있다.
- 하네스 커밋 메시지에는 아래 intent 표가 있다. 표는 커밋 메시지와 PR 본문에만 둔다. 기준은 AGENTS.md 손대지 말 것.
- 자기 커밋을 ACCEPT하지 않는다. 작성자와 리뷰어는 AGENTS.md 허용 범위대로 다른 주체다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

고정 base에서 작업 브랜치를 만들고, 그 경계의 검사를 실행한 뒤 결과대로 커밋한다. 실패는 실패로 적는다. 검사 선택은 `fvoci-fast-verify` 완료 조건.

## 손대지 말 것

- PR 브랜치 push와 자기 변경의 수락 판정은 하지 않는다. 통합 조건은 `fvoci-handoff` 완료 조건.
- 시크릿·credential·접속 URL·host·봇 식별자는 커밋과 보고 밖에 둔다.

## 출력

```
<한 줄 요약>

intent:
| 경로 | 이유 |
| --- | --- |
| <repo 상대 경로> | <이 변경이 지키는 계약> |

검사: <명령> exit <code>
```

```
fix: 문서 목록 빈 상태 문구

intent:
| 경로 | 이유 |
| --- | --- |
| apps/web/src/vue/documents/List.vue | 빈 목록에서 안내 문구를 보여 준다 |

검사: bun run lint exit 0
```
