# Orca 환경·도구 검증 기록

2026-09-24: 준비 검증 완료, 첫 인증·프로필 수직 기능 수락. 제품 검증 SHA·명령·잔여 기능은 `docs/rewrite.md`가 정본이다. 역할/소유권 규칙은 `AGENTS.md`를 따른다.

## 실제 실행 확인

| 역할·도구 | 실제 경로·설정 | 확인 근거 |
| --- | --- | --- |
| Orca | `/home/kinesis/.local/bin/orca-ide`, 1.4.207 | skills list, orca-cli/orchestration live guide, Run/Task/Dispatch 호출 성공. PATH의 `orca`는 빈 파일이므로 사용하지 않음 |
| 코디네이터 | codex0.156.1, `gpt-6-astra`, medium | 현재 cwd의 turn_context model/effort 대조 |
| 자문 | Claude Code2.1.280, `claude-fable-5-1`, medium | 실제 세션 JSONL/프로세스와 요청·유효 launch receipt 일치. Cursor Fable로 잘못 시작한 초기 task는 검토로 불인정 |
| 구현 | cursor-agent2026.09.18-9a7762b, `composer-2.5` | 목록·receipt·TUI·완료 보고 대조. 별도 effort 미지정 |
| 조사·검증 | 같은 cursor-agent, `cursor-grok-4.6-high` | 설치 목록의 Grok4.6 비-Fast ID, receipt/TUI 대조. 이름에 포함된 high 외 별도 추론 옵션을 요구하지 않음 |
| Rust | 1.98.1, cargo/rustfmt/clippy | 프로젝트 전용 설치; rustup-init1.28.2 공식 SHA256 대조. cargo test 사용 |
| PostgreSQL/Docker | PostgreSQL18.3, Docker29.8.1, native x86_64 | digest 고정 이미지와 실제 DB·HTTP 검사 |
| GitHub | 인증된 gh | 원본/PR999/CI 읽기, 대상 기존 PR1 확인. 원격 쓰기 없음 |

`agent`와 `cursor-agent`는 동일 설치 실행 파일임을 확인했다. Cursor turnStart 관측은 unsupported였으므로 receipt만으로 완료를 추정하지 않고 실제 TUI·반환 보고를 확인했다. 재사용 receipt의 model:null은 모델 재선택을 의미하지 않으며 동일 process incarnation의 검증된 Composer를 재사용했다.

## 저장소·공통 스킬

- 통합 worktree: `/home/kinesis/orca/workspaces/fvoci/daggertooth`, `fvoci/daggertooth`, 대상 AISFlow/fvoci. 초기97a3fe61 및 기존 starter 파일을 보존했다.
- 원본: 별도 clone `/home/kinesis/orca/references/fvoci-rust-source-20260924`, PR HEAD393795261322b916e588043cf94feca999175843 detached/clean. 기준 main/PR base는 rewrite 문서에 고정했다. 원본과 대상은 서로의 linked worktree가 아니다.
- 참조 clone에 쓰기 비트를 제거했다. 같은 사용자 권한으로 되돌릴 수 있는 우발 쓰기 방지이며 보안 샌드박스가 아니다. 워커는 현재 개발 계정 권한을 상속했으므로 숨겨진 도구만으로 강한 격리를 주장하지 않는다.
- 5개 프로젝트 스킬의 정본은 `.agents/skills`. Codex 목록 발견 및 명시 읽기, Cursor/Claude Code 실제 세션의 명시 읽기와 완료 반환 확인. 자동 탐색 성공을 다른 클라이언트에 전용하지 않았고 adapter/복제본을 만들지 않았다.
- 공식 스킬은 설치된 orca-cli/orchestration을 그대로 사용했다. 별도 scheduler/daemon/agent MCP는 만들지 않았다. 기존 다른 Run·전역 설정 불변.
- 원본 private / 대상 public을 확인했다. 공개 문서 조회에는 라이브러리명/버전만 사용했다. 이번 작업에서 push·PR 생성·원본 변경은 하지 않았다.

## 도구·자원

개발 MCP 추가 **0개**. GitHub는 gh, 문서는 공식 docs.rs/crates.io/도구 문서, 파일/Git/Rust는 기존 로컬 도구를 사용했다. 기존 MCP 설정은 덮어쓰지 않았다. 선택 MCP 미연결은 준비 차단으로 취급하지 않았다.

이 호스트의 Rust 실행 환경:

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
```

전역 셸 설정은 변경하지 않았다. worktree별 target, 실행별 UUID DB/비특권 앱 역할, 실제 port0 bind를 사용했다. crate 다운로드 캐시만 재사용했다. Redis/검색/브라우저는 첫 기능에서 필요하지 않아 기동하지 않았다. 동시 쓰기 최대2, 무거운 전체 검사 한 묶음으로 유지했다. 최종 두 실제 서버의 DB/역할/포트 분리 및 SIGTERM·재시작 보존 확인. 전체 workspace RLS/연결 풀 컨텍스트 검증은 후속 workspace API 수락 조건이다.

DB 검사는 `scripts/start-test-postgres.sh`로 재현할 수 있다. Docker 로컬 daemon, openssl, 고정 PostgreSQL digest를 사용하며 loopback 동적 포트·실행 label·모드600 임시 credential 파일을 생성한다. 테스트 명령의 성공/실패 후 컨테이너와 credential 정리를 실제 확인했다. 테스트 본문/빠른 경로는 설치나 네트워크 다운로드를 수행하지 않는다.

## task·검토·정리

Run: `run_b01d432a9dee`. 세부 허용 경로/명령/수락 조건은 각 Orca task spec이 정본이다.

| 작업 | task / dispatch | 결과 |
| --- | --- | --- |
| 초기 Claude Code 자문 | task_9f44ee133c43 / ctx_2988b16d4927 | 설계 검토 완료; user_takeover로 retained, 건드리지 않음 |
| 원본 계약 | task_988a8245f639 / ctx_cb0170f995ac | Grok 조사 완료, release |
| 협업·문서 probe | task_1532bb645f74 / ctx_00b1baf38504 | Grok e94ac2d 제출·통합 검증, release |
| 첫 제품 구현 | task_3377f29385d5 / ctx_6f56b812c5f0 | Composer6a77f76 제출, release |
| 보안 검토 | task_43a2bfe9a062 / ctx_f447be92fcf1 | Fable 차단 결함 확인, release |
| 첫 보강 | task_12716d8c1cc0 / ctx_cf69c321c975 | Composer e2a7380/31dda85 보존; 후속으로 소유권 이관 |
| HTTP/비밀번호 대조 | task_6678e958475f / ctx_d8b2fe334de7 | Grok 조사 완료, release |
| 수락 보강 | task_0f2b86c05fc9 / ctx_f3fb566d9a40 | Composer a240805 제출, release |
| 최종 독립 검토 | task_4aced35f7177 / ctx_ee0d5cd5054f | Fable a240805+25235f3 첫 slice 차단 해소 확인, release |

한 번의 재사용 시도 ctx_cdfbbb6d1ad2는 이전 follow-up 실행 중 readiness timeout으로 실패했다. 기존 실행 종료를 확인한 뒤 동일 task를 재시도했다. 오래된 capability로 보낸 추가 완료는 거부됐으며 성공 증거로 사용하지 않았다. 완료된 작업을 idle 상태만으로 수락하지 않았다.

최종 `worker-list --terminal-state reclaimable`은 빈 목록. retained4개는 초기 Fable user_takeover, 이관된 과거 readiness/보강 row 및 자원이 생성되지 않은 실패 시도다. 별도 진행 중인 쓰기 워커 없음. 원본·대상 task worktree와 커밋은 삭제하지 않았다.

이번 코디네이터 소유 `fvoci-rust-b01d432a9dee` 컨테이너와 admin-url/postgres.env credential 파일은 검증 후 제거했다. 시험 HTTP 프로세스도 종료 확인. 로컬 진단 아티팩트는 `/tmp/fvoci-rust-b01d-jpu9ngdq`에 남겼으며 credential 실값은 저장하지 않는다. 확인 당시 별도로 존재한 `fvoci-rust-test-pg-kinesis` 컨테이너는 생성 주체를 확정하지 못해 삭제하지 않았다. 그 컨테이너는 최종 수락 검사에 사용하지 않았다.

## 재개

AGENTS → 이 기록 → docs/rewrite 최신 수락 → 해당 Run task와 실제 git status/worktree/프로세스를 대조한다. 마지막 검증 코드195a58f, 다음 workspace API/인가·RLS task를 아직 배정하지 않았다. 세션 종료 시 미수락 제품 diff 없음. 다음 명령은 `cargo fetch --locked`, 빠른 gate, `scripts/start-test-postgres.sh`; 기존 DB/credential 파일을 재사용하지 않는다.

## 2026-09-24 재개 확인

Orca1.4.207 기존 실행 경로와 Run을 유지했다. Composer2.5 / Grok4.6 / Claude Code Fable5.1 medium의 신규 dispatch 요청·유효 모델을 대조하고 실제 TUI 작업을 확인했다. 새 MCP·전역 설정 변경 없음. 사용자 승인 범위는 AGENTS.md의 대상 원격 반영 절을 따른다. PR1은 검증 후 merge했고, 후속 통합 worktree는 `rust-workspace-integration` (main fe30bd1 기반)이다. 최신 수락·task 상태는 docs/rewrite.md에 둔다.

초기 rhwp Git dependency fetch가 공용 Cargo cache 잠금을 잡아 다른 worktree의 cargo clippy도 대기했다. target 출력은 분리되어 있으며 cache 대기를 컴파일/테스트 시간과 구분한다. 소유 불명 기존 PostgreSQL과 GitHub runner 컨테이너는 건드리지 않는다.


사용자 추가 승인: Fable5.1의 실제 주간 한도 도달 시 Claude Code Opus5.5
medium으로 자문을 대체할 수 있다. 아직 한도 도달/Opus 모델 ID·선택을
확인하거나 전환 실행하지 않았다. 현재 검토는 Fable5.1 medium이다.

## 2026-09-24 Fable 한도와 승인된 자문 전환

고정0f862f8 lifecycle 자문 task08b0ddbb932f/ctx601e47738d17의 실제 Claude Code
응답은 "You've reached your Fable limit. Run /usage-credits to continue or switch
models with /model."였다. 검토 보고서/worker_done 없이 종료되어 검토 완료로
인정하지 않았으며, 해당 dispatch를 worker-stop으로 중단·터미널 종료했다.
설치 실행 파일은 /home/kinesis/.local/share/claude/versions/2.1.281이며, 같은
한도 안내에는 "included Fable usage for this week"가 포함돼 있다. 설치된
모델 카탈로그에서 claude-opus-5-5를 확인했고 CLI --effort medium 지원도
확인했다. 사용자가 승인한 이 조건의 fallback만 적용한다. Opus 요청/유효
receipt와 실제 검토 반환은 아래 후속 기록으로 확인하며, 아직 완료로 주장하지 않는다.
/usage 읽기 전용 조회 외에 usage credits 활성화·결제·전역 설정 변경은 하지 않았다.

재개 receipt: 같은 task08b0ddbb932f의 ctx9edaf2220d96, Claude Code
`claude-opus-5-5`, medium 요청/유효 일치, turnStart observed. 실제 terminal
term582f3fbb-7a5a-416d-921b-3ee6e6ebb2ac의 해당 고정 SHA 읽기 전용 검토만
재개했다. 이전 Fable-only task 문구는 이 검증된 사용자 승인 fallback으로
명시적으로 대체 전달했다. 검토 완료 여부는 docs/rewrite 최신 결과를 따른다.

## 현재 자문 배정 — 사용자 전체 교체 지시

기존 Fable 역할을 모두 Claude Code Opus5.5 medium (`claude-opus-5-5`)으로
교체한다. 주간 한도 확인은 더 이상 전환 조건이 아니다. 기존 검토의 모델·결과
이력은 변경하지 않는다. 실행 경로는 검증된 Claude Code2.1.281이며, 앞선
ctx9edaf2220d96 및 ctx2297b3adb75a의 요청·유효 Opus/medium 검증을 유지한다.
새 검토도 실제 dispatch 설정을 대조한다. 코디네이터·구현·조사 역할은 불변이다(13:00Z 이전 기록; 코디네이터는 아래 절에서 교체).

## 코디네이터 교체 (2026-09-24 13:00Z 이후)

사용자 지시로 코디네이터를 Claude Code2.1.281 `claude-opus-5-5`, medium으로 교체했다.
실제 세션 모델 ID `claude-opus-5-5[1m]`(같은 모델의 1M context 변형이며 대체 모델이 아님), `/effort medium` 설정을 확인했다. Orca Run
`run_b01d432a9dee`에 `run-use`로 이 terminal(term_7123bc52)을 바인딩했다. 위 표의 codex
Astra 코디네이터 행은 **과거** 기록으로 보존한다. 독립 자문은 별도 Orca dispatch의 Claude Code
`claude-opus-5-5`/medium이며 launch requested/effective를 대조한다. 첫 재개 dispatch:
Grok `ctx_c97ebb20c9b0`(requested/effective `cursor-grok-4.6-high`), Composer
`ctx_cfdccc3dbce5`/`ctx_a796df4ecf21`(requested/effective `composer-2.5`).

## 상시 코디네이터 자문 (2026-09-25)

사용자 지시로 PR 검토와 별개인 상시 자문 세션을 둔다: Orca task `task_20fbb8c98686` /
dispatch `ctx_cd611507a373`, Claude Code `claude-opus-5-5` medium(requested/effective 일치), 읽기 전용.
코디네이터가 dispatch로 질문을 보내고 필요하면 terminal로 깨운다. 답은 Run 메일(`ADVICE:`)과
`/tmp/fvoci-advisor/*.md`. 워커·검토자 완료 전송은 반드시 `/home/kinesis/.local/bin/orca-ide`
절대 경로로 한다(PATH의 bare `orca`는 빈 파일이라 메시지가 조용히 유실된다).

## 2026-09-25 임시 All-Opus 실행 체제

사용자 지시로 모든 신규 AI dispatch는 Claude Code Opus 5.5 medium이다. Orca
`worker-start --agent claude --model claude-opus-5-5 --effort medium`의 launch receipt에서
requested/effective 모두 `claude-opus-5-5`/`medium`임을 확인했다(예: ctx_a7cca29eeb44).
진행 중이던 cursor Composer(가져오기·내보내기)·Grok(S3) 작업은 WIP 커밋·인계 기록 후
종료하고 Opus 워커가 같은 브랜치에서 이어받는다. 과거 실행 기록은 수정하지 않는다.

## 2026-09-26 역할 재배정 (사용자 통합 지시)

사용자 지시로 AGENTS.md의 역할 표를 교체했다. 이전 All-Opus 강제 규칙은 현재 운영 규칙에서 해제하고
당시 실행 기록은 위 절에 그대로 둔다. 확인한 실제 실행 경로:

| 역할 | 실행 경로·모델 | 확인 근거 |
| --- | --- | --- |
| 코디네이터 | Claude Code 2.1.283, `claude-fable-5-1`, `/effort medium` | 현재 세션 `/model`·`/effort` 출력, Run `run_b01d432a9dee`에 `run-use`로 terminal `term_77898e46` 바인딩 |
| 주 구현 | Orca `worker-start --agent claude --model claude-opus-5-5 --effort medium` | 첫 dispatch `ctx_07898f6011b7`(#85 수정) receipt requested/effective 모두 `claude-opus-5-5`/`medium` |
| 조사·검증 | Orca `worker-start --agent cursor --model cursor-grok-4.6-high` | 첫 dispatch `ctx_6612f5d79ee2`(읽기 전용 감사) receipt requested/effective `cursor-grok-4.6-high`, effort null(모델 ID에 포함, 별도 옵션 없음). `cursor-agent --list-models`에 Grok 4.6 계열 확인 |
| 독립 검토 | Orca `worker-start --agent claude --model claude-fable-5-1 --effort medium`, 별도 세션 | 첫 검토 dispatch receipt를 아래 후속 기록으로 확인한다 |

인계 시점: main `90a3df02`(#84), 열린 PR #81(`2d858c2`)·#82(`e95261e`)·#85(`c475fab`), 미푸시 task-api
`982f8637`, 활성 Opus 워커 collab-join-flake(`ctx_3a6ef72c2404`). 진행 중 프로세스를 강제 종료하지
않았고 기존 코드·검토·측정 근거는 그대로 재사용한다. 코디네이터 인계 메모는
`/home/kinesis/orca/fvoci-evidence/coordinator-handoff-2026-09-26.md`.

## 2026-09-26 CodeGraph (선택적 개발 탐색 도구)

- 사용자 지시로 코드 탐색·호출 관계·영향 조사 보조로만 도입한다. 새 모델·독립 검토자가 아니며 제품 런타임의
  Node 예외 승인이 아니다. Cargo.toml·제품 package.json·Docker 이미지·CI required check에 넣지 않는다.
- 설치는 이미 있던 `~/.codegraph/versions/v1.6.0`(`~/.local/bin/codegraph` 셸 래퍼, 번들 Node)이다. 릴리스
  v1.6.0(2026-08-26, tag commit `dfccdf62`)의 `codegraph-linux-x64.tar.gz` SHA-256
  `de3391f7…16b0`가 `SHA256SUMS`와 일치하고, 추출 내용이 설치본과 동일(`diff -rq` 차이 없음)함을 확인했다.
  GitHub attestation API에 SLSA v1 provenance 1건(release.yml, 빌드 commit `b59023f0`=tag의 부모)이 있다.
  gh 2.46에는 `gh attestation verify`가 없어 API 조회로 대신했다. 자동 upgrade는 켜지 않았다.
- telemetry: `codegraph telemetry off`로 `~/.codegraph/telemetry.json` `enabled:false`, 대기열 삭제. MCP 서버
  env에 `CODEGRAPH_TELEMETRY=0`·`DO_NOT_TRACK=1`을 넣었다(`telemetry status`가 DO_NOT_TRACK 우선을 표시).
  `update-check.json`은 남아 있어 수동 실행 시 버전 확인이 갈 수 있다.
- Claude Code 연결: `claude mcp add codegraph -s local …`로 daggertooth 프로젝트 범위(`~/.claude.json`)에만
  등록했다. `codegraph install`(전역·자동 허용·지침 삽입)은 쓰지 않았다. 노출 도구는 기본 `codegraph_explore`
  1개이며 stdio 초기화·tools/list·실제 explore 호출을 확인했다(0.2 s, 응답 25 KB ≈ 6k 토큰).
  cursor-agent(Grok)는 전역 `~/.cursor/mcp.json` 변경 없이 검증된 CLI(`codegraph explore|callers|impact`)를 쓴다.
- 인덱스: daggertooth(624 파일, 15.9k 노드, 66.9k 엣지, 4 s, 피크 RSS 1.2 GB, DB 74 MB). `target/`·
  `node_modules/`는 미포함. `.codegraph/`는 `.gitignore`·`.dockerignore`(PR #89)와 git info/exclude로 제외.
  worktree마다 별도 인덱스이며 `.codegraph`를 링크·복사하지 않는다. 검토자는 고정 SHA checkout에서 필요하면
  따로 init한다. HEAD·git status와 staleness 배너를 함께 본다.
- 정확도 관찰(사례 A·B·C): explore "ConvertClient callers"는 convert.rs·document_body·import_body·admin·
  export·project_documents를 찾았으나 share.rs(공개 PDF)·integrations.rs(AI 요약)는 누락했다(감사 E표는 rg로
  발견). `callers link_for_user`는 "없음"을 반환했으나 실제 호출이 src/oidc/flow.rs에 있다(메서드 호출 엣지
  누락). `impact ObjectStorage`는 157 심볼 후보를 반환했다. 따라서 결과는 조사 후보이며 SQL·RLS·cfg·IPC·
  trait dispatch 경계와 보안·삭제 결론은 실제 코드와 검사로 확인한다. 서버가 주입하는 "grep으로 재검증하지
  말라" 지침은 이 프로젝트의 검토·보안·데이터 보존 원칙을 대체하지 않는다.
