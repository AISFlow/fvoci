---
name: fvoci-handoff
description: "커밋을 제출·검토·통합하거나 작업을 재개·정리·종료를 판단할 때 쓴다. SHA·검사·남은 위험을 완료 조건으로 두며, 제품 구현과 시각 설계 절차를 대신하지 않는다."
---

# 제출·검토·통합

## 완료 조건

- 제출 보고에 base SHA, 커밋 SHA(여럿이면 순서), 작업 브랜치, 커밋별 파일, 실행한 명령, cwd, 범위·개수, PASS/FAIL/NOTRUN/MISSING, exit code, 남은 위험이 있다. 실행하지 않은 검사는 미실행이다.
- 리뷰는 그 고정 SHA의 diff·호출자·계약·검사에 대한 ACCEPT 또는 REQUEST_CHANGES다. ACCEPT에는 검토한 SHA가 적힌다. 검토 문장은 원격 CI를 대신하지 않는다. SHA가 바뀌면 바뀐 범위를 다시 본다.
- 통합 push는 그 커밋 객체의 fast-forward다. push된 head는 ACCEPT(2인이 필요한 경로는 2/2 ACCEPT)된 SHA와 같고, 부모는 직전 head다. 하나라도 다르면 push하지 않는다. push 후 head SHA, tested merge SHA, push한 주체를 알린다. 예전 SHA의 CI 성공을 새 head의 완료로 적지 않는다.
- 0.x(1.0.0 미만, 0.9.x 포함) main 병합은 아래가 모두 맞을 때 가능하다. 1.0.0 도달 또는 영환님 철회 전까지의 상시 조건이다.
  - 독립 리뷰어 두 명의 ACCEPT. 두 사람이 ACCEPT한 최종 head SHA를, 게이트 5개가 그 SHA를 검사한 뒤에 팀 방에 먼저 게시한다.
  - 그 head의 check run 전체(최신 attempt, `filter=latest`, 모든 페이지). `filter=all`은 쓰지 않는다. pull_request check run은 PR head에 붙는다.
  - 게이트 5개(`rust-ci-gate`, `web-ci-gate`, `install-ci-gate`, `documents-ci-gate`, `collab-engine-ci-gate`)는 conclusion success만 PASS다. FAIL, CANCELLED, SKIPPED, NEUTRAL, TIMED_OUT, NOTRUN, MISSING은 병합을 막는다.
  - 그 head의 다른 check run은 status `completed`이고 conclusion `success` 또는 `skipped`일 때만 통과다. neutral, stale, failure, cancelled, timed_out, startup_failure, action_required, 그리고 completed가 아닌 status는 막는다. 경로 미해당·Release·Turso처럼 check run이 없는 workflow는 제외한다. commit status(CodeRabbit, Renovate)는 병합 조건이 아니다.
  - 게이트가 검사한 tested merge SHA는 `TESTED_SHA`, `--tested-sha` 로그, 또는 checkout 로그 `HEAD is now at <sha> Merge <head> into <base>`에서 읽는다. API `head_sha`로 이 값을 대체하지 않는다. 그 SHA의 첫 부모는 현재 main SHA이고, 그 SHA는 현재 `refs/pull/N/merge`다. 아니면 새 push나 새 run으로 새 tested merge에서 게이트를 다시 돌린다. `gh run rerun`은 원래 `github.sha`를 재사용한다.
  - 머지 큐를 쓰면 큐 head를 게시하고, `merge_group` checkout SHA는 그 main 커밋 SHA와 같다.
  - 순서: 조건 확인 → gh 계정 `fvoci`로 Ready 전환 → `gh pr merge` 직전에 base와 게시한 head 재확인 → `gh pr merge --merge --match-head-commit <게시한 SHA>`. squash·rebase는 쓰지 않는다. 만든 merge commit의 부모는 `[확인한 main, 게시한 head]`다. SHA가 아니라 부모로 본다. 이 병합의 Ready 전환과 그 `Closes #N` 이슈 종료는 이 조건에 포함된다.
- Turso dispatch는 그 head의 나머지 필수 CI가 PASS한 뒤, 정확한 40자 SHA로 한 번이다. run의 `head_sha`가 요청 SHA와 다르면 결과는 MISSING이다.
- 재개할 때 원격 브랜치, PR head, CI를 다시 조회한다. 로컬 작업 트리에만 있는 커밋은 수락 근거가 아니다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

역할은 이렇게 나눈다. 리드는 계획·배정·지시·취합·보고를 하고 PR 브랜치를 push하지 않는다. 작성자는 허용 경로만 작업 브랜치에 커밋하고 SHA·파일·검사·exit code를 보고하며, 자기 변경을 수락하지 않는다. 리뷰어는 고정 SHA를 보고 ACCEPT 또는 REQUEST_CHANGES와 근거·최소 수정안을 돌려준다. 통합은 ACCEPT된 커밋을 그대로 fast-forward한다. CI 기록은 현재 head의 job 결과를 적고, 워크플로를 재실행·취소하지 않는다. tracer는 UI → 인가 → backend commit → ACK → 새 클라이언트 readback이 어디서 끊겼는지를 receipt·로그로 알린다.

작성자가 남긴 회귀 검사는, 리뷰어가 계약 근거와 실제 실패 포착(필요하면 변조)을 확인한다. 인가·저장 ACK·동시성은 거부·유실·경합과 실제 시스템 근거를 대조한다.

문서 커밋과 코드 커밋은 head를 나눠 CI를 따로 기록한다. 진행 중인 작업이 있으면 상태를 확인하고 같은 작업을 다시 시작하지 않는다. 한도·권한으로 멈춘 역할은 자동 반복·대기 프로세스·계정 우회 없이 리드에게 알린다. 정리할 때는 그 작업이 소유한 커밋·미추적 파일·프로세스만 다룬다. 수락 전에는 보존하고, 수락 후에도 커밋이 PR 브랜치에 있는지 확인한다.

PR 수락·머지는 포팅 종료가 아니다. 의존성이 갖춰진 다음 기능은 최신 main의 후속 작업으로 잇는다. 전체 완료는 기능·UI 연결, 보안·데이터·복구, 지원 DB·플랫폼, 배포 산출물의 필수 검증과 독립 검토가 main에 수락됐을 때다.

## 손대지 말 것

- 리뷰어는 검토 중인 커밋을 고치지 않는다. 통합은 cherry-pick·patch로 SHA를 바꾸지 않는다. 작성자와 리뷰어는 다른 주체다.
- 작성자는 PR 브랜치를 push하지 않는다. 원본 GitHub는 읽기 전용이다. main에 직접 push하지 않는다.
- 진행 중 CI를 직접 취소하지 않는다. `gh run rerun --failed`로 일부만 다시 돌리지 않는다.
- force push, `reset --hard`, 원격 브랜치 강제 삭제, 광범위한 kill·prune을 하지 않는다. 되돌림(revert)은 영환님 말이 있어야 한다.
- auto-merge를 켜지 않는다. 0.x 병합 순서 밖의 Ready 전환, 그 병합의 `Closes`가 아닌 이슈 종료, 태그, 릴리스, 배포, 시크릿, 패키지 공개 범위, 유료 사용, 권한 확대는 영환님 말이 있어야 한다.
- 사용자 승인 없이 계정·인증·결제, 상주 프로세스, scheduler, routine, MCP 서버를 새로 만들지 않는다. 멈춘 다른 작업 체인을 다시 켜지 않는다.
- 시크릿·cookie·credential·접속 URL·host·전체 환경을 보고·커밋·artifact에 남기지 않는다.
- 지정된 도구나 모델을 조용히 바꾸지 않는다. 대체는 알린 뒤 영환님 승인이 있을 때만 한다.
- 한 경로에는 작성자 한 명이다. manifest, lockfile, toolchain, workflow, migration 순서, 공유 API, 에이전트 문서, `apps/web/src/vue/{main.ts,App.vue,router.ts}`, `packages/i18n/src/locales/`는 담당이 정해진 뒤에 고친다. 소유하지 않은 경로까지 formatter·generator를 돌리지 않는다.
- 실행마다 DB·Redis prefix, 검색 index, 스토리지, 브라우저 profile, report를 나누고 port 0에 bind한 뒤 실제 포트를 전달한다. 작업 트리를 보안 격리라고 하지 않는다.
- 수락 전 probe를 제품 협업 지원으로 적지 않는다. opt-in·후속 분류로 수락 범위에서 빼지 않는다.
- 이 스킬에 특정 PR·SHA·run·로컬 절대 경로·현재 목록을 복제하지 않는다. 과거 승인·역할·모델은 현재 권한으로 승계하지 않는다.
