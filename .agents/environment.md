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
