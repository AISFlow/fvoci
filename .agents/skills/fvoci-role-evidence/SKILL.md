---
name: fvoci-role-evidence
description: "사람이 증거 역할을 명시해 부를 때만 쓴다. 현재 head에서 흐름이 처음 끊긴 단계를 receipt로 알린다."
disable-model-invocation: true
---

# 증거

## 완료 조건

- 판정은 현재 head의 UI → 인가 → backend commit → ACK → 새 클라이언트 readback 중 처음 끊긴 단계 하나다.
- 각 확인한 단계에 receipt 또는 로그 한 줄이 있다. exit 0만으로 흐름을 수락하지 않는다. 역할 경계는 `fvoci-handoff`의 tracer와 같다.
- 없는 단계는 MISSING이다. 결과를 이전 SHA와 합치지 않는다. AGENTS.md 완료 조건.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

단계 순서대로 근거를 찾고, 처음 끊긴 곳에서 멈춘다. 그 다음 단계는 미확인으로 적는다.

## 손대지 말 것

- 코드를 고치지 않는다. 워크플로를 재실행하거나 취소하지 않는다.
- 시크릿·credential·접속 URL·host·봇 식별자는 receipt에 남기지 않는다. AGENTS.md 손대지 말 것.

## 출력

```
head: <40자>
lane: <문서 저장 | 권한 | 협업>
끊긴 단계: UI | 인가 | backend commit | ACK | readback | 없음
근거: <receipt 또는 로그 한 줄>
다음: 미확인 | 해당 없음
```

```
head: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
lane: 문서 저장
끊긴 단계: ACK
근거: backend commit 로그는 있다. 클라이언트 ACK receipt는 없다.
다음: 미확인
```
