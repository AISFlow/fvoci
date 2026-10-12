# Orca에서 리드 맡기

Orca IDE로 에이전트를 운영할 때 FVOCI 역할을 Orca 구조에 맞추는 방법이다. 이 문서는 FVOCI 규칙이 Orca에서 어떻게 적용되는지만 다룬다. 명령 사용법은 Orca 자체의 orchestration 스킬과 `orca orchestration <명령> --help`가 정본이다. 환경 값(실행 파일 경로, 모델, 호스트 경로)은 레포에 적지 않는다.

## 코디네이터와 역할

Orca에서는 Run을 만들거나 Run에 바인딩한 세션이 코디네이터다. 코디네이터는 리드(`fvoci-role-lead`)를 맡는다. 즉 배정, 리뷰어 생성, 결과 회수, 보고를 한다.

코디네이터가 같은 세션에서 다른 역할도 맡을 수 있다. 그때는 다음을 지킨다.

- **병합·태그:** 통합 역할(`fvoci-role-integrator`)의 일이다. 보고에 통합 역할로 수행했다고 적고, `fvoci-handoff` 완료 조건을 그대로 따른다.
- **커밋:** 작성자 역할(`fvoci-role-author`)의 일이다. 코디네이터가 직접 커밋했다면 그 커밋의 리뷰어가 될 수 없다. 리뷰어는 다른 주체로 따로 띄운다.
- **보고:** 행동마다 어느 역할로 했는지 나눠 적는다. 한 세션이 했다고 해서 역할의 경계와 승인 조건이 합쳐지지 않는다.

## Orca 구조와 FVOCI 절차

| FVOCI 절차 | Orca |
| --- | --- |
| 리뷰 worktree | Orca worktree. 검토할 SHA를 base로 만들고 setup은 건너뛴다. 사이드바에 보이므로 숨은 실행 대신 쓴다. |
| 리뷰어 시작 | 그 worktree의 터미널에서 에이전트를 시작한다. `worker-start`로 Run의 감독 대상 worker로 등록한다. spec에는 프롬프트 파일 경로와 보고서 경로, 완료 조건을 적는다. |
| 완료 알림 | 리뷰어가 보고서를 쓴 뒤 `worker_done`을 `--report-path`와 함께 보낸다. 요약 첫머리에 판정과 짧은 SHA를 적는다. |
| 회수 | `check --wait`로 받고 delivery를 `--ack`한다. 보고서를 직접 연 뒤 `worker-release`로 놓고 터미널을 닫는다. |
| 리뷰어에게 추가 지시 | `send --to dispatch:<id>`. 터미널 handle은 바뀔 수 있으니 dispatch로 보낸다. |

## Orca에서 놓치기 쉬운 것

- **대기자는 하나만.** Run 하나에 `check --wait` 대기자를 하나만 둔다. 둘이면 같은 delivery를 나눠 받거나 상태 파일을 덮어쓴다.
- **foreground 대기 금지.** 대기는 백그라운드로 한다. 코디네이터 세션을 foreground 대기로 붙잡지 않는다.
- **heartbeat·status 메시지.** 이것만 담긴 delivery는 ack하고 계속 기다린다. 완료로 세지 않는다.
- **ack.** 받은 delivery는 반드시 ack한다. ack하지 않으면 같은 메시지가 다시 온다. 이력을 훑는 조회(전체 목록이나 시점 이후 조회)는 ack하지 않으므로 대기 수단으로 쓰지 않는다.
- **판정 확인.** `worker_done` 요약만 보고 판정을 세지 않는다. 보고서에 판정 줄과 40자 SHA가 있어야 한다(`review-dispatch.md`의 회수).
- **회수 순서.** 끝난 worker는 release하고 터미널을 닫는다. 남은 터미널과 worktree는 자원을 쓰고 사이드바를 어지럽힌다. 다음 라운드에서 다시 쓸 worktree만 남긴다.
- **메시지 인용.** `send`나 `ask`의 본문을 셸 큰따옴표로 넘길 때 백틱과 `$()`를 넣지 않는다. 셸이 먼저 실행한다.
- **실행 파일 확인.** 같은 이름의 다른 명령이 PATH에 있을 수 있다. 실제 Orca CLI가 실행됐는지 결과로 확인한다.
- **재시작 대비.** Run id, 프롬프트, 대기 상태 파일은 재시작해도 남는 디스크에 둔다. tmpfs에 두면 재시작할 때 잃는다.
- **무거운 명령.** worktree마다 빌드 디렉터리가 따로 생기므로, 리뷰어를 동시에 많이 띄우면 메모리가 빨리 찬다. 무거운 명령은 `review-dispatch.md`대로 호스트 lock 하나를 거친다.
