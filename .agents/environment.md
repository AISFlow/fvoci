# FVOCI 실행 환경과 도구

이 문서는 관측한 모델·Run 연결·설치 도구와 탐색 설정·실행 제약의 정본이다.
실행 경로 선택은 AGENTS.md를 따르며 아래 호스트·Run snapshot을 모든 환경의 필수 조건으로 적용하지 않는다.
역할·권한·소유권·병렬 배정 규칙은 [AGENTS.md](../AGENTS.md), 기능·후보 SHA·검사 결과·활성 Task와
다음 행동은 [rewrite.md](../docs/rewrite.md), 설치·복구는 [RUNNING.md](../RUNNING.md)를 따른다.
과거 모델 교체·PR별 일지를 현재 실행 절차에 섞지 않는다.

## 1. 과거 관측한 코디네이터와 Run 연결

아래는 2026-10-01 03:43 KST의 terminal 환경·세션 JSONL·설치 CLI·run-current를 읽기 전용으로 대조한 snapshot이다.
다른 세션이나 재개 시점의 현재 연결·권한·도구 가용성을 보장하지 않는다. 실제 환경을 다시 확인하고 기존 Run을 보존한다.

| 항목                     | 실제 값                                                                       | 근거                                                                         |
| ------------------------ | ----------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| 코디네이터 모델 / effort | `gpt-6.1-sol` / `high`                                                        | 최신 `turn_context`; 요청값·TUI 이름만으로 추정하지 않음                     |
| 실행 경로 / CLI          | Orca Codex 주 terminal, 세션 CLI `0.159.1` / 설치 CLI `0.159.2`, source `cli` | session_meta의 기존 세션 버전·03:43 `codex --version`의 설치 버전 구분       |
| 주 세션                  | `01a0f2c1-0b9e-7453-80a6-3e490b6ef7f0`                                        | 실제 `CODEX_THREAD_ID`와 JSONL 일치                                          |
| terminal handle          | `term_cc010fa2-f24c-4018-820f-16172de0e39f`                                   | 실제 `ORCA_TERMINAL_HANDLE`와 Run coordinator_handle 일치                    |
| 당시 Run / generation    | `run_496803f4d94f` / `2`                                                      | 본인 terminal의 기존 Run 바인딩 및 최신 run-current                          |
| Orca runtime             | `73201137-ed1f-4a8a-bcde-302a44c54e4b`                                        | 현재 CLI 응답 `_meta.runtimeId`                                              |
| 통합 worktree            | `/home/kinesis/orca/workspaces/fvoci/f272-batch-integration`                  | 최신 turn_context.cwd·실제 Git                                               |
| 당시 sandbox / approval  | `danger-full-access` / `never`                                                | 최신 turn_context; filesystem·network 접근 가능, 강한 읽기 전용 sandbox 아님 |

실제 transcript:
`/home/kinesis/.codex/sessions/2026/09/30/rollout-2026-09-30T23-38-52-01a0f2c1-0b9e-7453-80a6-3e490b6ef7f0.jsonl`.

사용자 전면 인계 후 본인의 attested terminal에서
`/home/kinesis/.local/bin/orca-ide orchestration run-use --id run_496803f4d94f --json`이
**2026-09-30T14:42:01Z** 성공했다. coordinator_handle은 위 본인이고 consumer_generation은 2다.
이전 generation 1의 대리 호출은 consumer_fenced로 거부되어 효과가 없었다. 기존 Run·작업·일곱 워커·WIP를
보존하고 공식 @all로 소유권 변경을 전달했다. 다른 Run 생성/reset·terminal 사칭·자동 체인 재가동은 없었다.
Run의 오래된 objective 문자열에 남은 “Astra”는 당시 생성 metadata이며 현재 역할 배정이 아니다.

이 snapshot 당시 구현·검증과 독립 검토는 Orca Codex terminal의 Sol 6.1이었다.
신규 실행에서는 선택한 런타임이 제공하는 시작·모델 근거를 확인한다. Orca launch는 requested/effective
`gpt-6.1-sol`과 effort, input_accepted/turn_started를 확인하고 보고서의 실제 session_meta/turn_context와 대조한다.
native subagent/workflow는 해당 런타임의 agent/run ID·시작 상태·실제 모델/effort 근거를 기록한다.
노출되지 않는 값은 미확인으로 남기며 Orca 전용 receipt나 transcript 경로를 다른 환경에 요구하지 않는다.
별도 컨텍스트·고정 SHA·검토 도구의 제한은 각 보고서에 기록한다. 지원하지 않는 hard read-only 제한을
적용했다고 주장하지 않으며 읽기 전용 prompt와 Git 변경 감시의 한계를 명시한다.
실행 불가·capacity·readiness 실패는 작업 시작·검토 완료로 세지 않는다. 모델 fallback·계정/결제 변경은 하지 않는다.

03:40의 워커 readiness 실패 복구 중 코디네이터가 업데이트를 건너뛰려 보낸 입력이 기본 업데이트를 실행해
CLI `0.159.2`가 실제 설치됐다. 의도하지 않은 변경과 첫 Task 미주입·공식 release/retry 근거는
기존 evidence의 `f272-codex-readiness-update-incident.json`에 보존한다. 모델 교체·계정/결제 변경은 없었고 추가 설치 변경은 하지 않는다.

2026-10-07 현재 ROOT의 기존 Run generation5와 승인 경계는 rewrite.md §1·§4 및 실제 task/dispatch로 확인한다. 위 옛 handle·PID·모델 snapshot은 현재 소유권이 아니다. 사용자가 명시한 Grok 감사/retry는 해당 고정 후보·배정 범위의 예외이며 기존 Sol 역할·계정·설정 변경을 뜻하지 않는다. 과거 포괄 merge/0.x 발행 승인은 AGENTS의 역사 기록으로만 읽고 현재 별도 승인 조건을 적용한다.

#272 후보 당시의 별도 승인 경계는 §6에 보존한다. 현재 Task/Dispatch·결과·다음 실행은
rewrite.md §1·§4의 관측 시각과 실제 Git·선택한 실행 경로·원격 상태를 대조한다.

## 2. 저장소와 설치 도구

다음 경로·설치 값은 §1과 같은 호스트 관측 기록이다. 다른 환경에서는 실제 가용 도구와 권한을 확인한다.

| 항목                 | 관측 당시 경로·버전 / 확인 범위                                                                                    |
| -------------------- | ------------------------------------------------------------------------------------------------------------------ |
| 호스트               | Linux `6.18.33.2-microsoft-standard-WSL2`, x86_64; 62 GiB RAM. 00:36 관측 가용35 GiB·디스크583 GiB는 일시 snapshot |
| Orca                 | `/home/kinesis/.local/bin/orca-ide`, **1.4.217**. 선택한 실행 파일을 모든 호출에 재사용                            |
| Codex                | 설치 `codex-cli 0.159.2`, 기존 주 세션 `0.159.1`; 모델/effort는 실제 transcript 확인                               |
| Bun                  | **1.4.2**, root `packageManager=bun@1.4.2`; 고정 lock와 worktree 로컬 dependencies 사용                            |
| 웹 도구              | manifest의 TypeScript5.9.3·ESLint10.11.0·Prettier3.9.9. Vue-tsc/Volar와 기타 pin은 manifest·patch 정본 확인        |
| Rust                 | 프로젝트 toolchain의 `rustc 1.98.1 (48a229cea 2026-09-01)`; cargo/rustfmt/clippy 경로는 아래 환경 설정             |
| Docker / Python / gh | Docker29.8.1, Python3.14.4, gh2.46.0. 해당 도구 버전 조회만 실행, 설치·전역 설정 변경 없음                         |
| CodeGraph            | `/home/kinesis/.local/bin/codegraph`, **1.6.0**; 번들 Node를 쓰는 개발 도구                                        |
| 원본 참조 clone      | `/home/kinesis/orca/references/fvoci-rust-source-20260924`; target와 별도 저장소. 원본 고정 SHA는 rewrite.md §1    |

원본은 private 읽기 전용 참조이고 대상은 AISFlow/fvoci다. 참조 clone의 쓰기 비트 제거는
같은 계정이 되돌릴 수 있는 우발 쓰기 방지이며 보안 sandbox가 아니다. 현재 개발 계정 접근을
Git worktree 분리나 숨겨진 도구만으로 강하게 격리했다고 주장하지 않는다.

이 호스트의 프로젝트 Rust toolchain을 선택할 때만 해당 command 환경에 다음을 적용한다.
현재 주 세션의 CARGO_HOME/RUSTUP_HOME은 unset이므로 자동 적용됐다고 추정하지 않는다.

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
```

프로젝트 설치의 최초 rustup-init1.28.2 공식 SHA-256 대조와 CodeGraph provenance는 §6의 과거 근거에 있다.
crate 다운로드 cache의 잠금 대기는 worktree target compile/test 시간과 구분한다.

## 3. 실행 경로별 절차·스킬·개발 연결

AGENTS.md에 따라 환경을 선택하고 실제 활성 작업을 이어간다. 다음 경로별 절차를 혼용하지 않는다.

### Orca를 사용하는 경우

- installed orca-cli/orchestration 스킬의 stub에서 선택한 binary의 live guide를 읽는다.
  필요할 때만 messaging/gates·coordinator-loop·placement/recovery reference를 추가로 읽는다.
  unsupported 명령을 추정하거나 bare `orca`로 조용히 전환하지 않는다. 과거 PATH의 bare orca 빈 파일 문제가 있었고
  §1 snapshot의 세션은 위 절대 경로로 통일했다. 현재 환경의 선택한 실행 파일은 다시 확인한다.
- worker-start receipt가 ready/turn observed인지 확인하고 uncertain/readiness timeout은 supported recovery로 조사한다.
  FIFO delivery의 모든 메시지를 처리하고 settled worker의 결과·잔존 자원·공식 release/retain 결정을 회수한 후 ACK한다.
  user_takeover retained를 released로 허위 기록하거나 다른 terminal·Run 자원을 강제 종료하지 않는다.
- Orca 작업은 해당 Run의 task/dispatch·worker 상태와 capacity/readiness를 확인한다. 한 시점의 실패를 영구 제한으로 일반화하지 않는다.

### native subagent/workflow를 사용하는 경우

- 현재 런타임의 지원되는 위임·메시지·결과 조회·재개 도구를 사용하고 agent/workflow ID와 소유 경로를 기록한다.
- 결과와 journal 등 실제 제공되는 인계 근거를 회수한다. Orca의 terminal handle·task receipt·ACK/release를 요구하거나 있다고 주장하지 않는다.
- 실제 서비스 슬롯 제한을 확인하고, 이를 Orca나 다른 환경의 영구 워커 상한으로 전용하지 않는다.

### 공통 확인

- 배정 규칙은 AGENTS.md가 정본이며 실제 platform/CPU/RAM/disk/DB/browser·통합 처리량을 관측한다.
  선택 경로가 불가하면 제한을 보고하며 환경·모델을 조용히 대체하지 않는다.
- 프로젝트 스킬 정본은 `.agents/skills/`다. 현재 catalog 발견과 필요한 SKILL.md의 명시 읽기를 구분한다.
  클라이언트의 자동 탐색 성공을 다른 클라이언트에 전용하지 않고 모델별 adapter/전문 복제본을 만들지 않는다.
- 개발 MCP·상주 scheduler·daemon·중복 agent MCP를 이번 인계/정리에서 추가하지 않았다.
  GitHub는 gh, 파일/Git/Rust는 로컬 도구, 공개 library 문서는 해당 공식 출처를 쓴다.
  기존 연결·전역 MCP 설정을 덮어쓰지 않으며 private 원본을 공개 문서 서비스에 전송하지 않는다.
- 과거 Claude 프로젝트 CodeGraph MCP 등록·Cursor CLI 실행은 당시 검증 이력이다.
  §1 snapshot의 Codex 코디네이터는 검증된 CodeGraph CLI를 직접 사용했다. 현재 환경의 연결은 다시 확인하며 과거 MCP 연결을 현재 세션 연결로 주장하지 않는다.

## 4. CodeGraph 설정과 정확도 한계

코드 탐색·영향 확인에는 AGENTS.md에 따라 설치된 CodeGraph를 활용한다. 아래는 확인된 CLI 경로이며,
미설치·호출 불가 시 제한을 보고하고 실제 source·검색·검사로 확인한 범위를 명시한다.
과거 MCP 호출명을 현재 도구 가용성으로 가정하지 않는다. worktree마다 자기 `.codegraph/` 인덱스를 사용한다. 관계·영향 탐색이 필요한 범위는 `status` → 필요시 `init -y`/`sync` →
`explore`/`callers`/`impact`로 조회하고 실제 source·SQL·RLS·cfg·IPC·trait dispatch·검사와 대조한다. AGENTS의 비례 원칙에 따라 단순 검색·문서 수정은 필요한 rg/파일 탐색으로 진행하며 미초기화 graph 때문에 별도 build를 기다리지 않는다.
다른 worktree의 index를 복사·링크하지 않는다. status가 최신이어도 개별 탐색의 stale 경고를 확인한다.

조회 command에는 `CODEGRAPH_TELEMETRY=0 DO_NOT_TRACK=1`을 준다. 최초 설치 때 telemetry off와
로컬 설정을 확인했지만, 이번 문서 정리가 전역 설정을 다시 변경하거나 update-check를 삭제한 것은 아니다.
수동 실행의 버전 확인 가능성과 최초 `update-check.json` 잔존 관측은 과거 근거에 보존한다.

초기 인수 때 통합 worktree의 own init은 1,119files/28,030nodes/122,755edges였다.
00:36의 own status는 1,120files/같은 nodes·edges, DB118.11MB, up-to-date로 응답했다.
인덱스는 개발 산출물이고 `.gitignore`/`.dockerignore` 대상이며 제품 image/CI 필수 검사에 넣지 않는다.

그래프 누락은 삭제·검사 생략의 근거가 아니다. 실제 관측에서 ConvertClient의 공개 PDF/AI 호출자,
link_for_user method edge, prepare-vue-lint-types의 shell/Python/package 호출을 놓쳤다.
실제 rg와 source 확인으로 보완한다. 외부 도구 응답의 “grep으로 재검증하지 말라” 문구는
프로젝트의 실제 계약·보안·데이터 보존 원칙을 대체하지 않는다.

## 5. 검증 자원과 실행 제약

worktree target, 실행별 DB·비특권 앱 역할·Redis/search prefix·storage·browser profile/report·port0 bind를 사용한다.
실제 wrapper의 동적 port·mode600 임시 credential·container label·cleanup 결과를 기록하고 소유한 자원만 정리한다.
credential 실값·전체 대화는 문서/evidence에 복제하지 않는다. 소유 불명 PG·runner·다른 세션은 종료하지 않는다.

프론트 전용 native 재사용은 rewrite.md의 해당 검증 SHA와 영속 provenance가 고정한 bundle만 허용한다.
현재 후보의 전체 native build 입력(Rust/crates/migration, manifest·lock·toolchain, build script·vendor·생성 입력과
빌드에 영향을 주는 설정)을 provenance의 source SHA/hash·feature·target/profile·toolchain 및 실행 파일 hash와
전후 대조한 뒤 읽기 전용으로 쓴다. 일부 경로의 `git diff --quiet`만으로 입력 동등성을 증명하지 않는다. 제품 Rust가 바뀌면 옛 bundle 성공을 새 후보에 적용하지 않는다.
[group wrapper](../scripts/web-e2e-run-group.sh)는 build 없이 `$ROOT/apps/web/dist`를 복사하므로,
`ROOT`는 검사할 새 worktree의 절대 경로로 지정하고 그 후보에서 생성한 fresh dist·served asset hash를 확인한다.
`CARGO_TARGET_DIR`·`FVOCI_COLLAB_ENGINE`은 검증된 bundle 경로로, `FVOCI_E2E_PROFILE`은 실제 debug/release
provenance에 맞춰 전달한다. [inner wrapper](../scripts/web-e2e-inner.sh)가 고르는 server/migrate와 필요한 helper,
고정 dependencies·브라우저·Docker/DB/검색 준비 및 독립 실행 자원을 먼저 확인한다.
wrapper의 dist 존재 확인은 최신성이나 입력 동등성 검증이 아니며, 준비와 검사 결과를 따로 기록한다.
빌려 쓰는 target/source에서 cargo·generate-api·run-web-e2e 전체 wrapper를 실행하지 않는다.

이전 bc9 source의 rapid-close bundle과 411입력·5 binary/mode/hash·db-tests/worker 근거는
rewrite.md §6의 과거 evidence 포인터에 보존한다. Closed outbound 수정 뒤의 own native와 최종 default-feature image는
별도 고정 입력·feature·metadata로 판단한다. 옛 prebuilt-8ada도 역사적 근거이며 현재 입력 검사를 대신하지 않는다.

현재 pinned wrapper의 local PostgreSQL18.3/Meili1.53.2·read-only native feature 근거와
원격 PG16/17/18·ARM/기타 CI 범위는 해당 실제 job/report대로 구분한다. 로컬 debug-feature 성공은
게시 image·production 배포·전체 플랫폼 성공이 아니다. 비용/perf는 같은 실행 조건으로 별도 측정한다.

Bun Playwright 실행은 upstream 지원 보장이 없고, 기존 실제 실행 결과만 유효하다.
Volar `@volar/typescript`2.4.28 Bun patch는 upstream 수정 전 버전 갱신과 함께 검사한다.
Bun unit timeout60초·XLSX hostile stream 비용·chunk advisory의 기존 제약을 숨기지 않는다.
strict lint의 SFC 선언은 pinned compiler로 생성하고 stale/failure output을 제거한다.
full-web emit TS2742·잘못 설치된 TS 버전 등을 any shim·rule 완화로 우회하지 않는다.

과거 1.4.207의 readiness/headless·desktop_activation_blocked 실패와 이후1.4.217 desktop/graph/visible
복구는 별도 사건이다. 현재 run/worker receipt 성공을 화면 focus나 매 순간 OS UI가 정상이라는 주장으로 확대하지 않는다.
실패와 support recovery·실제 runtime 모델 증거는 보고서에 남긴다.

### 5.1 현재 Run의 영속 evidence root

현재 WSL 실행에서 코디네이터가 정한 root `E`는 `/home/kinesis/orca/fvoci-evidence/v060-20261004/coherent-final-ci-root/`다. 2026-10-07 관측: 실제 절대 경로·uid1000 소유·쓰기/탐색 가능, root mode0755이며 비공개 task/output 하위 경로의0700·파일0600 계약은 별도로 적용한다. 기존 root/실행 경로·봉인·Grok 배치 바인딩을 변경하거나 공통 권한을 조정하지 않는다.

이는 현재 환경의 값이며 모든 사용자/호스트/CI의 고정 설치 경로가 아니다. 다른 환경은 코디네이터가 명시한 접근 가능한 영속 절대 경로를 사용하고 task의 root·허용 하위 경로·기존 출력 인자를 대조한다. 필수 root 누락·불가에는 실행을 시작하지 않고 보고하며 `/tmp`나 cache로 자동 대체하지 않는다. 경로와 보존 위치가 명시된 기존 CI staging/artifact handoff는 원래 literal 경로·수락/영속 보관 계약을 유지한다; 임시 output 존재만으로 근거 보존을 완료했다고 하지 않는다.

전달 절차·기존 인자 의미는 [handoff의 evidence root 절차](skills/fvoci-handoff/SKILL.md#영속-evidence-root와-명시적-전달)를 따른다. task 명세의 root/허용 하위 경로와 실제 프로세스에 전달한 값이 근거이며, 부모 shell 환경이 워커에 자동 전달됐다고 가정하지 않는다. `FVOCI_EVIDENCE_DIR`는 capacity probe의 기존 로그 옵션이고 selected는 `FVOCI_SELECTED_CI_OUTPUT`/필수 `--output`, allocation의 `outputRoot`/`runRoot` 및 handoff 절대 경로·소유자·0700을 유지한다. 새 공통 환경변수나 evidence framework는 추가하지 않는다.

`target/collab-probe-logs`의 capacity 기본값과 upgrade-smoke의 임시 기본값은 개발/역사 명령의 configurable default이며 실제 로그를 재생성 가능한 cache로 분류하지 않는다. 이번0.6 활성 실행에 자동 사용하지 않고 실제 필요 시 기존 명시 옵션으로 연결한다. 과거 개발 upgrade-smoke는 현재0.6 gate가 아니며 기본값 개선은 별도 후속이다. 과거 고정 evidence 절대 경로는 §6·고정 보고서에 보존하고 일괄 치환하지 않는다.

## 6. 과거 기록과 재개 포인터

#272 전환 당시에는 Vue 후보 URL 연결 → Rust/API/DB·production 브라우저 검증 → 독립 검토·CI → main 수락
순서와 별도 사용자 승인 전 머지·태그·릴리스·제품 배포 금지를 적용했다. 흐름별 React 제거 후 공통 React 부팅·
의존성·빌드 구성을 제거했다. #272는 main `67c3e19ab953169131013bcc2753dfb7ac39229c`에 머지됐으며,
이전 후보 상태·명령은 역사적 근거다. 이 사실은 새 릴리스·배포 승인이나 미완료 검사 수락을 부여하지 않는다.

정리 전 전체 환경 기록은
[고정 f442의 environment.md](https://github.com/AISFlow/fvoci/blob/f442a9f06c438b51524e13cb7a2043ff5d95566a/.agents/environment.md)와
`git log -p -- .agents/environment.md`에서 확인한다. 과거 Astra/Fable/Opus/Grok/Composer/Sol 역할·fallback·수량 상한,
Ultracode 요청/실제 effort·capacity/readiness 실패·old Run·dispatch/실행·정리 이력은 당시 사실로 보존한다.
과거 모델을 현재 역할에 재배정하거나 실행 명령을 현재 재개 절차에 다시 넣지 않는다.

- 2026-09-30 인수와 보고서: `/home/kinesis/orca/fvoci-evidence/recovery-20260930/takeover-evidence/`.
- 2026-10-01 두 문서의 보존 delta·coverage·검토: 같은 root의 `sol-coordinator-docs-20261001/`.
- 과거 cleanup/cache·614GiB 회수 근거: `/home/kinesis/orca/fvoci-evidence/space-reclaim-2026-09-29/` 및 recovery result/receipts.
- 최초 CodeGraph1.6.0 release/provenance: f442의2026-09-26 설치 기록, release tag `dfccdf62`, build `b59023f0`,
  tar SHA256SUMS 일치·attestation API 조회. gh2.46의 attestation verify 미지원은 API 조회와 구분한다.
- 과거 Run `run_b01d432a9dee`와 당시 native/workflow 세션은 추적 자료다. 이 이력은 신규 작업의 native/workflow 사용을 금지하지 않으며 재개 대상은 실제 활성 상태로 판단한다.

재개는 AGENTS → 이 파일의 관측 기록과 현재 실제 연결/도구 → rewrite.md §1·§4 → 실제 Git·선택 경로의 task/worker 또는 workflow/agent·CI 순서다.
이미 회수한 코드·검토·실패 근거를 재사용하고 현재 질문·남은 delta부터 이어간다.
2026-10-01 정리는 재인수·재구현·전수 감사가 아니었다. 모델·계정·권한 배정은 유지하며, 실제 CLI 설치 변경은 §1의 근거와 구분해 기록한다.

### 2026-09-30 모델 인계의 과거 맥락

모델 배정은 2026-09-30의 Sol 6.1 high 인계를 기준으로 하되 최신 사용자 지시를 우선한다. 당시 Astra의 배정·통합·원격 쓰기 중단과 이전 임시·전용 배정의 교체는 인계 이력이며, 모든 환경에서 반복할 절차가 아니다. 기존 Run·활성 워커·작업·근거를 보존하며 과거 모델·검토·검증 이력은 이 문서와 Git 이력에 보존한다. 모델 변경만으로 기존 구현을 재작성하거나 전수 감사를 반복하지 않는다. 과거 8개 작성자·2/3개 검토자 제한과 모델별 수량 면제는 현재 상한이 아니다.
