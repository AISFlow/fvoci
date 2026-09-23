# Orca 환경·도구 검증 기록

상태: **준비 검증 진행 중, 2026-09-24**. 모델 목록과 launch receipt를 대조했고 읽기 전용 완료 반환을 확인 중이다. PENDING은 미검증이다.

## 실행 환경

| 항목 | 확인값 | 근거/상태 |
| --- | --- | --- |
| Orca 버전·실행 파일 | 1.4.207, `/home/kinesis/.local/bin/orca-ide` | live guide 및 runtime ready 확인. PATH의 `orca`는 0바이트 파일이므로 사용 금지; 전역 파일 수정 없음 |
| 원본 저장소·기준 SHA | docs/rewrite.md의 고정 SHA | 별도 clone `/home/kinesis/orca/references/fvoci-rust-source-20260924`, PR HEAD detached; 기존 원본 개발 checkout 불변 |
| 대상 경로·통합 SHA | `fvoci/daggertooth`, 초기 97a3fe61 | 기존 미추적 starter 파일 보존. 제품 수락 없음 |
| codex 실행 파일·Astra 실제 ID·medium | PENDING | 요청값/유효값 대조 |
| Fable 실행 경로·실제 ID·medium | Claude Code 2.1.280, `--model fable --effort medium` | ctx_2988b16d4927 receipt 일치; 실제 5.1 모델 확인 진행. 잘못 실행한 Cursor Fable ctx_abdf08f0a2aa는 지정 검토로 불인정 |
| cursor-agent 실행 파일·Composer 2.5 ID | 2026.09.18-9a7762b, composer-2.5 | ctx_866afd0e5dff 요청/적용 일치 및 실제 완료. agent와 cursor-agent는 동일 설치 파일 |
| cursor-agent 실행 파일·Grok 4.6 ID | cursor-grok-4.6-high | 목록 표시명 Grok 4.6, 비-Fast; 별도 effort 지정 없음. ctx_6ca17e6bc982 요청/적용 일치 및 완료 |
| 공식 Orca 스킬 출처·버전 | 설치된 1.4.207 live guide | skills list, skills get orca-cli, orchestration --full 및 필요한 reference 조회 |
| Rust toolchain·Cargo·nextest 사용 여부 | PENDING | 실제 설치/호환성 |
| 프로젝트 스킬 로딩 | PENDING | Codex/Cursor/Fable 경로 각각 확인 |
| task 조정 경로 | Run run_b01d432a9dee | 공식 Run/Task/Dispatch; 기존 다른 Run 불변. 모든 워커 prompt에 검증된 CLI 절대 경로 명시 |
| 격리·샌드박스 실효 범위 | Git/자원 논리 격리; 보안 샌드박스 아님 | 현재 사용자 개발 권한 상속. OS가 원본 쓰기/자격 증명 접근을 차단한다고 주장하지 않음 |

실제 로컬 절대 경로·세션 ID·호스트 정보가 민감하거나 다른 머신에서 무의미하면 이 표에는 논리 이름과 검증 상태만 남기고, 상세값은 Orca 세션 기록 또는 명시적으로 gitignore된 로컬 기록에 둔다. 토큰/쿠키/DB 비밀번호는 어느 쪽에도 출력하지 않는다.

## 초기 자원 정책

쓰기 워커 최대 2개, 하위 워커 포함. Fable은 필요 시 읽기 전용 자문. 무거운 전체 검증은 같은 프로젝트/호스트에서 한 묶음. 이는 초기 운영값이며 변경 시 측정 근거를 남긴다.

산출물: worktree별 target. 데이터·포트·임시 파일·브라우저 profile/report: 실행별 격리. 다운로드 캐시는 재사용하되 산출물과 혼동하지 않는다. 실제 앱/DB가 생성된 첫 기능에서 간섭 없는 동시 실행을 확인하기 전에는 해당 검증을 완료로 표시하지 않는다.

## 개발용 MCP 선택

| 도구 | 기본값 | 활성화 조건 | 제한/대체 경로 |
| --- | --- | --- | --- |
| Orca 작업 조정 | 공식 CLI/스킬 | 기존 설치 사용 | 자체 MCP 서버를 만들지 않음 |
| GitHub | 기존 연결/gh 우선 | 부족하면 공식 github/github-mcp-server | 원본 읽기 전용, 필요한 repos/issues/PR/Actions만 |
| Context7 | 선택·미활성 | 버전별 문서 조사에 도움이 될 때 | 공개 질문만, 공식 문서/컴파일로 재확인 |
| Playwright MCP | 선택·미활성 | CLI/테스트보다 탐색적 브라우저 제어가 필요할 때 | 로컬 테스트 profile, 기존 테스트로 회귀 고정 |
| DB/Filesystem/Shell/Git/메모리 MCP | 추가하지 않음 | 중복 아닌 구체적 필요를 입증할 때 재검토 | 네이티브 도구·테스트 DB 사용 |
| CodeGraph 등 코드 탐색 | 추가하지 않음 | 기존 승인 설치와 언어 지원·효용을 확인한 경우 | rg/LSP/원본 파일 조회로 대체 |

위 목록은 일괄 설치 목록이 아니다. 기존 도구로 작업이 되면 추가 MCP는 없어도 된다. 기존 사용자 설치를 임의로 삭제하거나 전역 비활성화하지 않는다.

실제로 채택한 도구에만 다음을 추가한다: 공식 출처, 고정 release/digest 또는 원격 endpoint, 설정 관리 위치, 연결을 사용하는 역할, 허용 도구·계정 권한, 자격 증명의 참조 이름, 무해한 실제 호출 결과, 실패 시 대체 경로. 원격 서비스는 서버 버전을 고정하지 못할 수 있으므로 고정했다고 주장하지 않는다.

Orca의 MCP 연결이 워커 CLI에도 로딩되는지 확인한다. JSON/TOML과 클라이언트 설정 탐색은 실제 도움말을 따른다. 잘못된 공통 schema·추정 CLI 옵션을 만들지 않는다. 새 브라우저 context로 개발 자격 증명을 격리하고, 민감한 저장소/기기 화면을 불필요하게 전송하지 않는다.

## 준비 확인

- [ ] 저장소·기준 SHA·기존 변경 보존 확인.
- [ ] 지정 모델과 확인 가능한 추론 강도 실제 실행 확인.
- [ ] 짧은 읽기 전용 task의 cwd/규칙/스킬 읽기와 완료 반환 확인.
- [ ] 필요한 도구의 실제 최소 권한 호출 확인.
- [ ] 작업 소유권과 자원 분리 규칙 확정. 런타임 검증은 구현 후 실시.
- [ ] task/dispatch 추적과 중단 후 재개 위치 확인.

준비가 끝나면 설정 탐색을 반복하지 말고 첫 제품 기능을 구현한다. 실행 파일/버전/권한이 바뀌면 영향받는 항목만 다시 확인한다.

## 현재 준비 진행

Rust 1.98.1을 프로젝트 전용 `/home/kinesis/orca/toolchains/fvoci-rust`에 설치 중. rustup-init 1.28.2의 공식 SHA256 대조 성공; 셸 PATH와 전역 설정 변경 없음. Docker 29.8.1 연결 확인. DB/port 격리 실증은 제품 검사 시 수행한다. 원본은 private, 대상 remote는 public이므로 비공개 소스나 계약 내용을 원격에 게시하지 않고 로컬 구현/검토만 수행한다. 선택 MCP 추가 없음.
