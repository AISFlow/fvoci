---
name: fvoci-fast-verify
description: FVOCI 변경의 검사를 고르거나 CI 실패·느린 테스트를 조사하고, 기존 구현과 Rust 구현의 검증 시간을 비교할 때 사용한다.
---

# 최소 충분한 검증

현재 task의 자원/소유권은 AGENTS.md와 환경 기록을 따른다. 실행하지 않은 명령과 아직 구현되지 않은 profile을 성공으로 적지 않는다.

## 입력

변경 diff와 SHA, 기존 테스트 명령, 필요한 불변식, 사용 가능한 격리 자원.

## 절차

1. 순수 정책 → 실제 DB → 내부 HTTP → 실제 기동/네트워크 → 브라우저/배포 중 어느 경계가 바뀌는지 정한다. 가장 직접적인 검사와 필수 연결 검사를 고른다. 공통 인가/DB/CI 변경은 범위를 넓힌다.
2. 빠른 경로에 외부 서비스·설치·네트워크 호출이 없는지 확인한다. SQLx 매크로를 쓰면 offline metadata로 일반 check가 DB에 기대지 않게 하고 별도 스키마 검사에서 metadata를 검증한다.
3. 실제 실행 명령을 그대로 기록한다. cargo check는 테스트 실행이 아니다. fast/DB 명령은 실제 대상/필터와 실행 개수를 확인한다. 선택이 잘못되어 테스트 0개를 실행한 것을 통과로 처리하지 않는다.
4. 실패하면 최초 실패와 stdout/stderr의 관련 부분을 보존하고 재현 범위를 줄인다. 제품 결함·환경 준비·자원 충돌·검사 준비 문제를 구분한다. blind retry, 거대한 sleep, skip, timeout 증액으로 숨기지 않는다.
5. 대기/컴파일/환경 준비/본문 실행/정리 시간을 가능한 범위에서 구분한다. 같은 장비·기능·실패 조건에서 초기와 warm 검사를 따로 비교한다. 적은 테스트를 빠르게 돌린 결과를 동등 성능이라고 하지 않는다.
6. 워커는 관련 검사만 수행한다. 전체 통합/E2E/이미지 검사는 코디네이터가 소유 자원을 배정한 실행만 수행한다. 실패 조사와 무거운 검증에 새 글로벌 큐를 만들지 않는다.

## 라이브러리 교체와 결과 재사용

[표준 구현 교체](../fvoci-standard-implementations/SKILL.md)는 공식 벡터·허용/거부 입력과 우리 adapter의
보안 설정·오류 의미, 실제 client/DB/UI 연결을 검증한다. SDK 내부 테스트 전체를 다시 복제하거나
SDK가 테스트됐다는 이유로 FVOCI 경합·복구 검사를 생략하지 않는다. 기존 버그와 명세가 다르면
원본 출력을 무조건 정답으로 고정하지 않는다. 바이너리·배포 입력이 바뀌면 설치 경계도 확인한다.
차등 검사는 차이 발견 수단이다. 바이트 일치는 암호학적 입력·원본 파일·실제 프로토콜/소비자 또는
명시된 계약에만 요구하고, 나머지는 내용·구조·서식·권한 의미를 비교한다. 원본 결함 snapshot은 올바른
기대 동작의 회귀로 바꾸되, 정규화로 누락을 숨기거나 현재 Rust 출력에 기대값을 그대로 맞추지 않는다.

동일 코드·동등 조건/범위의 원격 성공은 재사용하고, 통합 후 바뀐 부분과 환경 차이만 필요한 만큼
추가 확인한다. HEAD/base/합성 SHA·feature/target·실제 테스트 실행을 대조한다. 필수 검사 누락을
성공으로 취급하지 않는다. bounded read-only polling은 허용하되 쓰기를 성공할 때까지 반복하지 않는다.

## 결과

검사명, 정확한 명령/cwd/SHA, 실행 범위/개수, 결과와 exit code, 소요 시간·조건, 생략 이유와 남은 위험을 반환한다. 누락된 DB나 브라우저 환경이 필요한 검사는 미실행/실패로 분명히 표시한다.

## Vue 정적 검사와 CI 정본

현재 Vue/TypeScript·typed ESLint·Prettier 명령과 준비 순서는
[Web CI의 `web-static` job](../../../.github/workflows/web.yml),
[루트 scripts](../../../package.json), [웹 scripts](../../../apps/web/package.json),
[editor scripts](../../../packages/editor/package.json)가 정본이다. 명령 목록을 별도로 복제하지 않는다.
고정 dependency 준비 후 lint fixture·lint·format과 웹/editor typecheck의 실제 실행 범위를 확인한다.
정적 검사는 Rust/DB build 없는 별도 job이며 API/unit·브라우저 수락을 대신하지 않는다.
무거운 로컬 검사는 AGENTS.md의 한 통제 배치에 모으고 자원에 맞춰 실행한다.
native 재사용은 [환경 기록 §5](../../environment.md#5-검증-자원과-실행-제약)의 provenance·fresh dist 조건을 따른다.

## Web Playwright CI shard (browser job only)

1. 정책·플래너: `python3 scripts/web-e2e-groups.py verify --shards 8` 와 `python3 -m unittest scripts.test_web_e2e_groups` 는 DB·브라우저 없이 실행한다. `apps/web/e2e/*.spec.ts` 만 정상 범위이며, 중첩·`.test.ts` 등 미지원 패턴은 플래너가 실패로 막는다.
2. 샤드 래퍼 고정 검사: `bash scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh` 가 stub 빌드·run-group으로 `--ci-shard` 플로우(플랜 선검증, 샤드당 빌드 1회, 그룹 실패 전파, 샤드 수 override 거부)를 검증한다. 프로덕션 래퍼에는 dry-run·디렉터리 override가 없다.
3. 통합 스크립트: `bash scripts/test-web-e2e-groups.sh` 가 위를 묶는다. CI `web-checks` 와 동일 명령을 로컬에서 먼저 돌린다.
4. 대표 브라우저 샤드( PostgreSQL·Chromium )는 코디네이터 배정 후 `bash scripts/run-web-e2e.sh --ci-shard N` 으로만 실행한다. 전체 8 샤드·원격 `web.yml` 은 통합 수락 경로에서 확인한다.

## CI selection planner/gate (workflow changes)

1. `bash scripts/test-ci-selection.sh` — `python3 scripts/ci_selection.py verify-workflows` 와 `python3 -m unittest scripts.test_ci_selection` 을 DB·브라우저 없이 실행한다. 플래너는 `scripts/ci_selection.py` 단일 구현만 사용한다. `plan` 은 레지스트리 검증을 출력 전에 실행한다. PR narrow 는 체크아웃 HEAD가 GitHub의 tested merge SHA와 같고 둘째 parent가 event head와 정확히 일치할 때만 허용한다. 첫 parent는 event base와 같거나 그 후손이어야 하며, 누적 PR diff와 첫 parent 대비 실제 merge 결과 diff를 합쳐 분류한다. 기준이 무관하거나 뒤로 이동한 경우·head 불일치·검증 실패는 전체 선택 또는 실패로 닫는다. workflow의 기본 merge checkout·전체 history·변조되지 않은 `GITHUB_SHA`와 항상 실행되는 PR gate를 레지스트리 회귀로 확인한다.
2. 각 `.github/workflows/{web,rust,documents,collab-engine,install}.yml` 의 `ci-plan` 은 pinned PyYAML(`scripts/ci_selection_requirements.txt`)을 설치한 뒤 `plan` 을 돌리고, `*-ci-gate` 는 항상 실행된다. Rust `ci-plan` 만 `bash scripts/test-ci-selection.sh` 를 plan 출력 전에 한 번 실행한다. 게이트는 `NEEDS_JSON=${{ toJSON(needs) }}` 와 tested SHA만 받으며 ci-plan 결과·plan_json·등록 job 결과를 그 객체에서 도출한다. 제품 job 은 plan 출력 boolean 으로만 skip 한다. matrix job 은 job-level `if` 로 통째로 skip 하며 빈 matrix 를 만들지 않는다.
3. 원격 Actions·merge_group·`workflow_dispatch` full 경로는 통합 수락에서 확인한다. 로컬에서는 플래너/게이트 단위 테스트만 최소 충분으로 돌린다.
4. 새 workflow/job을 추가하면 `WORKFLOW_JOBS`·`WORKFLOW_YAML`, 해당 plan 출력과 gate의 `needs`를 함께 갱신하고 등록 누락 부정 회귀를 유지한다. 새 Web spec은 shard 자동 발견 결과를 확인한다. 새 Rust integration target은 Cargo 등록뿐 아니라 실제 CI의 양쪽 아키텍처 실행 목록에도 포함한다. 실행 목록 통합 시 기존 target을 빠뜨리지 않았는지 비교하고, 변경 선택 규칙 자체는 전체 CI로 검증한다.
5. `verify-workflows`는 루트 Rust DB integration target의 Cargo·자동 발견 목록과 PostgreSQL/S3/협업 실행 경로도 대조한다. 새 target은 실제 실행 목록에 연결하고 `bash scripts/test-ci-selection.sh`로 등록 누락·실행 조건을 확인한다. fast/native 또는 수동 probe 예외는 해당 실행 근거를 확인해 기존 분류에 명시하며, 이 DB 등록 검사를 전체 Rust 검사 범위의 증거로 쓰지 않는다.
