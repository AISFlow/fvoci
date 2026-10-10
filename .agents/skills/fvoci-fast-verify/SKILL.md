---
name: fvoci-fast-verify
description: "바꾼 경로의 검사, 실패 원인, CI 수락을 고를 때 쓴다. 그 경계의 명령이 exit 0인 것과 결과 구분을 완료 조건으로 두며, 무관한 전체 제품 검사를 요구하지 않는다."
---

# 검사 선택과 CI 수락

## 완료 조건

- 바꾼 경계의 명령이 exit 0이다. 순수 정책, DB, HTTP, 기동·네트워크, 브라우저·배포 중 실제로 바뀐 경계만 고른다.
- 보고에 검사명, 명령, cwd, SHA, 범위·개수, PASS/FAIL/NOTRUN/MISSING, exit code, 소요, 생략 이유, 남은 위험이 있다. 0개 실행, cargo check, 미실행 DB·브라우저는 통과가 아니다.
- `.github/workflows/` 또는 `scripts/ci_selection.py`가 바뀌면 `bash scripts/test-ci-selection.sh`가 exit 0이다. 이 명령은 `verify-workflows`와 `python3 -m unittest scripts.test_ci_selection`이다.
- 웹 샤드 정책을 바꾸면 `bash scripts/test-web-e2e-groups.sh`가 exit 0이다. 이 명령에 포함된 verify·단위 검사를 별도로 중복 실행하지 않는다.
- 반복 횟수는 AGENTS.md 손대지 말 것이다.
- 원격 수락은 그 head의 CI다. 같은 코드·같은 범위의 원격 성공은 재사용하고, 바뀐 부분만 더 본다. 리뷰 판정은 그 CI를 대신하지 않는다.

## 기본 절차

1. diff와 기존 테스트에서 가장 직접적인 검사와 필수 연결 검사를 고른다. 공통 인가·DB·CI가 바뀌면 범위를 넓힌다. 문서만 바뀌었다고 제품 검사 전체를 추가하지 않는다.
2. 빠른 경로에 외부 서비스·설치·네트워크가 없는지 본다. SQLx 매크로는 offline metadata로 일반 check가 DB에 기대지 않게 하고, metadata는 별도 스키마 검사에서 확인한다.
3. 실패하면 최초 실패와 stdout/stderr를 남기고 재현을 줄인다. 제품 결함, 환경 준비, 자원 충돌, 검사 준비 문제를 구분한다. 대기·컴파일·준비·본문·정리를 나눠 적는다. 같은 장비·실패 조건에서 초기와 warm을 따로 비교한다.
4. 작성자는 관련 빠른 검사만 한다. 전체 통합·E2E·이미지는 배정된 실행이거나 통합 head의 원격 CI다.
5. 라이브러리를 바꿀 때는 공식 벡터, 허용·거부 입력, adapter의 보안 설정·오류, 실제 client/DB/UI를 본다. SDK 내부 테스트 전체를 복제하지 않고, SDK가 테스트됐다는 이유로 FVOCI 경합·복구를 빼지 않는다. 차등 비교는 차이 발견용이다. 바이트 일치는 암호 입력, 원본 파일, 실제 프로토콜·소비자, 명시된 계약에만 요구한다.
6. 웹 정적 명령의 정본은 `web-static` job, 루트 `package.json`, `apps/web/package.json`, `packages/editor/package.json`이다. 명령 목록을 여기에 복제하지 않는다. 정적 검사는 API·unit·브라우저 수락을 대신하지 않는다.
7. native bundle은 같은 실행 안에서 아키텍처, feature, target, profile, toolchain, native 입력 전체를 hash로 대조할 때만 재사용한다. 경로 일부의 `git diff --quiet`나 디렉터리 존재는 입력 동등성이 아니다. `scripts/web-e2e-run-group.sh`는 `$ROOT/apps/web/dist`를 복사하므로 `ROOT`는 그 작업 트리의 절대 경로다.
8. CI 플래너 변경은 `scripts/ci_selection.py` 한 구현만 쓴다. PR narrow는 체크아웃 HEAD가 tested merge SHA와 같고 둘째 parent가 event head와 같을 때만 된다. 누적 PR diff와 첫 parent 대비 merge 결과 diff를 합쳐 분류한다. 게이트는 `NEEDS_JSON=${{ toJSON(needs) }}`와 tested SHA에서 plan·job 결과를 읽는다. 제품 job은 plan boolean으로만 skip한다. 새 job은 `WORKFLOW_JOBS`, `WORKFLOW_YAML`, plan 출력, gate `needs`를 함께 갱신한다.

## 손대지 말 것

- 타임아웃·retry·sleep·skip·flaky·REJECT·재실행·무료 티어·이전 SHA 합산은 AGENTS.md다. 쓰기를 성공할 때까지 반복하지 않는다. bounded read-only polling은 된다.
- full-web emit TS2742나 잘못된 TypeScript 버전을 any shim·rule 완화로 넘기지 않는다. strict lint의 SFC 선언은 pinned compiler로 만들고 stale 출력을 지운다.
- Bun Playwright는 실제 실행 결과만 유효하다. `@volar/typescript` Bun patch는 그 버전 갱신과 함께 검사한다. Bun unit timeout 60초, XLSX hostile stream 비용, chunk advisory를 숨기지 않는다.
- 게이트 `paths:`는 AGENTS.md. 대표 브라우저 샤드는 배정 후 `bash scripts/run-web-e2e.sh --ci-shard N`으로만 실행한다.
