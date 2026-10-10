---
name: fvoci-runtime-boundaries
description: "Node 서버 경로, 설치 준비, Cargo 바이너리, native child, 배포·빌드 경계를 바꿀 때 쓴다. 제품 런타임과 설치 계약을 완료 조건으로 두며, 그 경계와 무관한 Vue·문서 수정에는 쓰지 않는다."
---

# 런타임·설치·child

## 완료 조건

- 바꾼 경계에서 실제 호출자, 프로세스, 권한, 종료·복구가 맞고, 그 검사가 exit 0이다.
- 결과에 유지·제거한 실행 경계와 이유, 검증 SHA, 측정 조건, 남은 runtime, 복구 범위가 있다.
- Node 제거를 수락하려면 Node/Bun/Deno와 내장 JS 엔진이 없는 최종 제품 환경에서 변환·공유·설치·복구를 실행한 근거가 있다.
- 배치를 바꾸면 호출자 갱신이 같은 커밋에 있다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

crate, 배포 실행 파일, 프로세스, 독립 service, 운영자 명령을 구분한다. 현재 build·이미지·호출 경로에서 그 변경만 찾는다. 그래프에 안 보이는 환경 변수·Command·IPC·SQL은 원문으로 보완한다.

분리가 라이브러리 재사용인지, 같은 실행 파일의 내부 모드인지, 별도 helper인지 확인한다. 기본 설치 준비(설정 검증, DB·검색 준비, migration, grant, 검색 키)는 앱 컨테이너 시작이 하고, 성공한 뒤에만 제한된 앱 역할로 서버를 시작한다. 정상 서버 프로세스에 소유자 credential·master key를 남기지 않는다. 다른 쓰기 서버가 살아 있는 동안의 schema 변경처럼 안전 조건을 확인할 수 없는 업그레이드는 거부한다. 실제 uid·파일 권한·환경·FD를 확인한다. exec 환경 필터는 컨테이너 설정값을 지우지 않으므로, `docker inspect`에 값이 없다고 주장하지 않는다. child 모드는 서버 초기화·credential 로딩·listen 전에 분기하고 설치 절차를 실행하지 않는다. 자식에는 필요한 환경만 주고 입력·출력·시간·메모리·동시성·취소·종료를 검증한다.

남아 있는 변환 호출은 `fvoci-standard-implementations`로 Rust 구현을 먼저 찾는다. 기존 TS는 격리된 개발용 oracle로 두고, 검증된 단위부터 호출을 바꾼다. Markdown/Tiptap·HTML·문서 출력의 구조·표·서식·한글·emoji·이미지·링크·인가·오류·한도를 보존한다. HTML은 sanitize, URL, CSP도 본다. 신규 가져오기의 CRDT 생성과 기존 문서 이력 보존을 구분한다.

의사결정에 필요한 warm build, clean 제품 build, 산출물 크기, child 시작·IPC·메모리·종료를 같은 조건에서 비교한다. 측정하지 않은 성능 개선을 주장하지 않는다. DB·협업 검사는 `fvoci-fast-verify`와 `fvoci-db-security`를 쓴다.

## 손대지 말 것

- 서버 측 제품 연산의 기준은 Rust다. 새 Node 의존, 숨은 JS fallback, 내장 JS 엔진, JS 런타임 번들, 외부 변환 서비스 우회를 넣지 않는다. Vue/Tiptap, 브라우저 JS, 개발용 Node/CodeGraph, TS oracle, 합의한 PostgreSQL·Meilisearch·S3·SMTP는 제품 서버와 별개다.
- 자식 프로세스는 `process.execPath`로 띄운다. rlimit·env_clear를 파일시스템·네트워크 sandbox라고 하지 않는다. timeout과 Drop만으로 정리 완료라고 하지 않는다. `spawn_blocking`만으로 process 격리를 대체하지 않는다.
- parser·CRDT process 격리를 유지한다. 위험한 파싱을 HTTP 프로세스 안에 넣지 않는다. 편집 중 문서를 JSON 왕복으로 다시 만들어 삭제·동시편집 이력을 버리지 않는다.
- 하네스(영환님, 2026-10-09 11:31 KST): 하네스 코드는 Rust(xtask)와 TypeScript(Bun)뿐이다. Python·shell 하네스 파일은 옮긴 뒤 제거한다. 이전은 intent부터 한다. diff·intent 표는 커밋 메시지와 PR 본문에 두고 레포 파일로 남기지 않는다.
- 영환님, 2026-10-10 10:39 KST: 새 `.py` 파일과 새 Python 코드는 레포에 두지 않는다. 하네스 이전이 그 파일을 대체하기 전까지 기존 Python 파일(예: `scripts/ci_selection.py`)을 고칠 수 있다. 그 수정의 경계는 2026-10-10 10:42 KST다. 기존 검사를 유지하거나 바꾸는 수정은 허용된다. 새 기능이나 새 테스트 준비 코드는 Python에 더하지 않고 TypeScript 또는 Rust에 둔다.
- 외부 Python 도구(2026-10-09 repowise 결정): uv로 설치해 도구로 쓸 수 있다. 그 도구 때문에 레포에 Python 코드를 더하지 않는다.
- 새 검증 wrapper를 만들지 않는다. 손 절차를 대체하는 예외는 지정 task의 xtask 하위 명령뿐이다.
- Docker/build 입력, 설치 CI, manifest/lockfile은 그 경로의 작성자가 정해진 뒤에만 맞춘다. 개발·비교 fixture의 JS를 무조건 지우지 않는다.
- migrate의 current_exe를 서버로 보지 않는다. PATH에서 node만 숨기고 번들 runtime을 남긴 것을 Node 제거로 세지 않는다. 운영 배포·서비스 추가·권한 확대는 이 스킬의 범위가 아니다.
