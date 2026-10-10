---
name: fvoci-role-integrator
description: "사람이 통합 역할을 명시해 부를 때만 쓴다. ACCEPT된 커밋을 fast-forward하고 head와 tested merge를 구분해 알린다."
disable-model-invocation: true
---

# 통합

## 완료 조건

- push한 head는 ACCEPT된 커밋 SHA와 같고 부모는 직전 head다. 다르면 push하지 않는다. 절차는 `fvoci-handoff` 완료 조건.
- 알림은 head SHA와 tested merge SHA를 서로 다른 줄에 적는다. 이전 SHA의 CI를 이 head의 완료로 적지 않는다.
- main 병합은 AGENTS.md 허용 범위가 성립하고 병합 SHA를 방에 먼저 게시한 뒤에만 한다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

push 직전에 커밋 SHA와 부모를 다시 읽는다. cherry-pick이나 patch로 커밋을 다시 만들지 않는다. push 뒤에 head와 tested merge를 다시 읽어 알린다.

## 손대지 말 것

- 커밋 내용을 고치지 않는다. 미검토 커밋은 push하지 않는다.
- force push와 `reset --hard`는 허용 조건이 없다. AGENTS.md 손대지 말 것.

## 출력

```
통합: push | 보류
head: <40자>
parent: <40자>
tested-merge: <40자 | 아직 없음>
근거: <ACCEPT SHA와 부모 일치 한 줄>
```

```
통합: push
head: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
tested-merge: cccccccccccccccccccccccccccccccccccccccc
근거: head는 ACCEPT된 SHA와 같고 부모는 직전 head다.
```
