---
name: fvoci-handoff
description: 모든 변경의 제출·독립 검토·통합·재개·자원 정리와 전체 포팅 종료 판단에 사용한다. 선택한 실행 경로의 인계를 다루며 제품 구현·시각 설계 절차를 대신하지 않는다.
---

# 제출·독립 검토·통합·재개·종료 판단

소유권·승인과 실행 경로 선택은 AGENTS.md를 따른다. 사용자가 지정한 환경을 우선하고 재개 시 실제 활성 실행과 부분 결과를 보존한다.
신규 작업은 현재 도구·권한으로 지원되는 경로를 선택한다. 제한이 있으면 보고하며 환경·모델을 조용히 대체하지 않는다.

- Orca: 공식 orchestration live guide와 실제 Run·task/dispatch·worker를 사용하고 완료 전송·ACK·release/retain 상태를 확인한다.
- native subagent/workflow: 해당 런타임의 위임·결과 조회·재개 절차를 사용하고 agent/run ID와 결과·제공되는 journal을 보존한다. Orca 전용 receipt·ACK/release를 가정하지 않는다.

두 경로 모두 같은 단일 작성자·고정 SHA 독립 검토·검증 수락·원격 승인 규칙을 따른다.

## 작업 제출

다음 최소 정보를 한 번에 전달한다.

- task/dispatch 또는 workflow run·agent ID, 실제 도구·모델·추론 설정·agent 유형, base/head SHA와 worktree.
- 변경 파일/커밋, 미커밋·미추적 파일과 보존 필요 여부.
- 검사 명령·실행 범위·성공/실패/미실행 및 관련 증거 위치.
- 남은 위험, 띄운 프로세스·DB/volume 등 소유 자원, 소유권 반납 여부.

사용하는 경로의 완료 전달 절차(workflow 결과·journal, Orca의 완료 전송·ack·release)를 따른다. idle이나 종료된 터미널은 기능 완료의 증거가 아니다. 전체 대화·시크릿은 저장하지 않는다.

## 영속 evidence root와 명시적 전달

현재 환경값은 [환경 기록§5.1](../../environment.md#51-현재-run의-영속-evidence-root)만 정본으로 두고 task마다 절대 경로를 명시한다. 코디네이터가 한 번 해결한 run root와 작성 가능한 task/시도 하위 경로를 task 명세에 넣어 전달하고, 워커는 받은 값·실제 경로·owner/쓰기 가능 여부와 명령의 기존 인자를 대조한다. 다른 사용자/클라우드/CI도 자신의 명시적 영속 보관 위치를 사용한다. 필수 root가 없거나 불가하면 실행 전에 실패를 보고하고 기본 `/tmp`·cache로 fallback하지 않는다; 환경만 지정하고 전달/수락이 자동 완료됐다고 하지 않는다.

기존 경로 옵션을 재사용하며 공통 자동 전파 기능이나 새 변수를 만들지 않는다. 아래는 호출 형태이며 실행 권한/준비 완료가 아니다. `task_evidence_dir`는 task에서 받은 새 시도의 경로이고, 각 명령의 생성/존재·권한 조건과 실제 allocation을 먼저 만족해야 한다.

```sh
: "${task_evidence_dir:?task evidence directory must be explicitly supplied}"
FVOCI_EVIDENCE_DIR="$task_evidence_dir" bash scripts/collab-capacity-probe.sh
python3 scripts/run-selected-backend-e2e.py record-before --output "$task_evidence_dir"
```

- Capacity probe만 기존 `FVOCI_EVIDENCE_DIR`를 로그 디렉터리로 읽는다. selected 명령은 필수 `--output`을 사용하고 shell caller가 `FVOCI_SELECTED_CI_OUTPUT`을 명시 전달한다; local allocation의 `outputRoot`와 runtime 하위 경로 `FVOCI_CI_SELECTED_RUNS`/`runRoot`가 일치해야 한다. 일반 Python entry는 output을 resolve하지만 Web handoff는 실제 절대 경로·실행 계정 소유·0700을 거부 조건으로 확인하므로 처음부터 명시 절대 경로를 준다.
- `FVOCI_WEB_BUILD_HANDOFF`/digest·source/feature/ABI·owner/allocation·uid/gid·CI/local 실행 gate를 그대로 유지한다. 이미 바인딩한 output/packet/native/input 경로를 새 root 규칙 때문에 이동·치환하지 않는다. explicit CI staging은 기존 handoff/artifact 보관으로 영속 근거를 연결하며 cache hit나 임시 output을 최종 보존 증거로 대신하지 않는다.
- 역사적 upgrade-smoke의 기존 `--evidence-dir "$task_evidence_dir"`는 해당 명령이 승인될 때만 사용한다. tmp 기본값은 현재0.6 신규 설치/현재 archive 복원 gate에 적용되지 않는다. 다른 명령의 기존 `--output`·run 경로·`FVOCI_PERF_OUT`·`FVOCI_NATIVE_IME_EVIDENCE`도 각자의 의미를 유지하며 통째로 새 변수로 대체하지 않는다.
- 최초 결과·실패·missing log·seal을 보존하고 retry에는 별도 시도 경로/ID를 준다. archive evidence와 재생성 가능한 cache를 구분하되 유일한 qualified binary/input·원 근거는 cache 정리 대상으로 간주하지 않는다. 원 실행과 결과·후속 명령을 연결하고, 필요한 관측 외 token/cookie/접속 URL/host·전체 환경·불필요한 session/transcript를 저장하지 않는다.

## 검토와 통합

코디네이터가 고정된 제출 SHA의 diff/계약/검사 결과를 확인한다. AGENTS.md 역할표의 독립 검토자는 필요한 설계·보안·협업 위험을 검토하며 차단 결함·근거·최소 수정안을 반환한다. 검토 문구가 실제 검사 결과를 대신하지 않는다.
구현자는 회귀 검사를 작성하되, 독립 검토자는 기대값의 계약 근거와 관측·판정 경로가 실제 실패를 포착하는지도 확인한다.
인가·저장 ACK·동시성처럼 위험한 경계는 성공 결과만 읽지 않고 관련 거부·유실·경합 시나리오와 실제 시스템 검증 근거를 대조한다.

코디네이터가 자기 통합 worktree에서 한 작업씩 반영한다. 검토 중 제출 SHA가 바뀌면 변경된 범위를 다시 본다. 통합 후 필요한 검사를 새 통합 SHA에서 실행한다. 미수락 코드나 예전 SHA의 성공을 완료로 기록하지 않는다.

## 세션 재개

AGENTS.md, 환경 기록과 현재 실제 연결, 선택 경로의 활성 Run·task/dispatch·worker 또는 workflow·agent, docs/rewrite.md의 최신 수락 SHA와 다음 작업을 확인한다. 실제 git status/worktree와 프로세스를 대조한다. 이전 실행이 남아 있으면 상태부터 확인하며 같은 작업을 다시 시작하지 않는다.

### Pending·한도 종료 후 인수

1. 현재 진행 문서가 가리키는 인계 파일에서 미푸시 SHA·로그·task/dispatch·소유권을 찾고 실제 로컬/
   원격과 대조한다. 로컬 HEAD와 원격 HEAD 차이를 소실로 오해해 reset하지 않는다.
2. 인박스에 도착한 결과(workflow는 결과·journal)와 실제 프로세스를 먼저 확인한다. Pending은 프로세스 종료, worker_done은
   검사/독립 검토 수락, CI 성공은 머지 완료의 동의어가 아니다. 이전 실행이 없거나 안전하게 인계됐음을
   확인하고 기존 부분 결과를 이어받아 작업당 한 실행만 재개한다.
3. 한도·권한이 복구되지 않았으면 그 역할의 자동 반복 호출·대기 daemon·모델/계정 우회를 만들지 않는다.
   역할/권한 변경은 AGENTS.md와 최신 사용자 지시를 따르고 과거 작성·검토 이력은 보존한다.
4. 로그의 SHA·명령·종료 코드·실행 범위를 읽고 유효한 결과를 재사용한다. 통합 delta의 필요한 검사와
   독립 검토만 추가한다. 과거 인계에 적힌 PR 번호·상태를 현재 상태로 고정하지 않는다.

이 스킬에는 특정 Run/PR/SHA·한도 초기화 시각·로컬 절대 경로·현재 Pending 목록을 복제하지 않는다.
역할/승인은 AGENTS.md, 설치 설정은 환경 기록, 현재 진행과 인계 포인터는 docs/rewrite.md에서 읽는다.
다음 세션용 프롬프트는 목표·인계 위치·현재 우선순위만 전달하고 스킬 전문을 다시 붙이지 않는다.

## 정리

해당 작업의 커밋·미추적 파일·로그·프로세스 소유권을 확인한 뒤 소유 자원만 정리한다. worktree/branch 강제 삭제, 광범위한 kill/prune, 다른 Run reset을 하지 않는다. 수락 전에는 보존하고, 수락 후에도 커밋이 유지되는지 확인한다.

## 완료 조건

제출과 수락 상태가 구분되고, 다음 세션이 재조사 없이 마지막 검증 SHA·남은 diff·다음 명령을 찾을 수 있어야 한다.

## 지속 진행과 전체 포팅 종료

PR 수락·머지는 전체 작업 종료가 아니다.
기능 대응표에서 의존성이 충족된
다음 사용자 기능을 선택해 최신 main의 후속 task로 계속한다. 현재 Vue 흐름과 문서 권한·저장·
Hocuspocus/Yrs 동시편집의 실제 수락 근거를 보존하고 남은 delta부터 진행한다.
수락 전에는 probe를 제품 협업 지원으로 표시하지 않는다.

고정 원본 기준과 승인된 최종 지원 범위는 docs/rewrite.md의 기능 대응표로 추적한다.
개별 task·PR 완료로 종료하지 않고 의존성이 준비된 다음 제품 기능을 이어간다.
전체 완료는 기능/UI 연결, 보안·데이터·복구, 지원 DB·플랫폼, 배포 산출물의
필수 검증과 현재 역할표의 독립 검토를 마치고 main에 수락됐을 때만 선언한다. 미구현·미연결·
부분 검증·원본부터 미구현을 구분하며 opt-in이나 후속 분류로 범위를 제외하지 않는다.
세션 한계에서는 기존 진행 기록에 검증 SHA·미수락 diff·활성 소유권·실패·다음
명령·CI 상태·잔존 자원을 남긴다. 설정되지 않은 백그라운드 실행을 약속하지 않는다.
