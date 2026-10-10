---
name: fvoci-runtime-boundaries
description: "CI 하네스·CLI를 Rust/xtask·Bun으로 이전하거나 설치·native child·배포·빌드 경계를 바꿀 때 쓴다. 호출·실패·종료 계약을 검증하며, 무관한 Vue·문서 수정에는 쓰지 않는다."
---

# 런타임·설치·child

## 완료 조건

- 바꾼 경계에서 실제 호출자, 프로세스, 권한, 종료·복구가 맞고, 그 검사가 exit 0이다.
- 결과에 유지·제거한 실행 경계와 이유, 검증 SHA, 측정 조건, 남은 runtime, 복구 범위가 있다.
- Node 제거를 수락하려면 Node/Bun/Deno와 내장 JS 엔진이 없는 최종 제품 환경에서 변환·공유·설치·복구를 실행한 근거가 있다.
- 배치를 바꾸면 호출자 갱신이 같은 커밋에 있다.
- 하네스 이전 완료에는 실제 CLI·CI 호출자와 lint/typecheck/test 등록이 새 구현을 사용한 검증, 대체된 옛 실행 경로 제거가 포함된다. 호출자가 옛 구현을 쓰는 companion·probe는 이전 완료가 아니다.

## 기본 절차

crate, 배포 실행 파일, 프로세스, 독립 service, 운영자 명령을 구분한다. 현재 build·이미지·호출 경로에서 그 변경만 찾는다. 그래프에 안 보이는 환경 변수·Command·IPC·SQL은 원문으로 보완한다.

분리가 라이브러리 재사용인지, 같은 실행 파일의 내부 모드인지, 별도 helper인지 확인한다. 기본 설치 준비(설정 검증, DB·검색 준비, migration, grant, 검색 키)는 앱 컨테이너 시작이 하고, 성공한 뒤에만 제한된 앱 역할로 서버를 시작한다. 정상 서버 프로세스에 소유자 credential·master key를 남기지 않는다. 다른 쓰기 서버가 살아 있는 동안의 schema 변경처럼 안전 조건을 확인할 수 없는 업그레이드는 거부한다. 실제 uid·파일 권한·환경·FD를 확인한다. exec 환경 필터는 컨테이너 설정값을 지우지 않으므로, `docker inspect`에 값이 없다고 주장하지 않는다. child 모드는 서버 초기화·credential 로딩·listen 전에 분기하고 설치 절차를 실행하지 않는다. 자식에는 필요한 환경만 주고 입력·출력·시간·메모리·동시성·취소·종료를 검증한다.

남아 있는 변환 호출은 `fvoci-standard-implementations`로 Rust 구현을 먼저 찾는다. 기존 TS는 격리된 개발용 oracle로 두고, 검증된 단위부터 호출을 바꾼다. Markdown/Tiptap·HTML·문서 출력의 구조·표·서식·한글·emoji·이미지·링크·인가·오류·한도를 보존한다. HTML은 sanitize, URL, CSP도 본다. 신규 가져오기의 CRDT 생성과 기존 문서 이력 보존을 구분한다.

의사결정에 필요한 warm build, clean 제품 build, 산출물 크기, child 시작·IPC·메모리·종료를 같은 조건에서 비교한다. 측정하지 않은 성능 개선을 주장하지 않는다. DB·협업 검사는 `fvoci-fast-verify`와 `fvoci-db-security`를 쓴다.

### CI 하네스·CLI 이전

언어부터 번역하지 않고 기존 목적, 실제 소비자, 입력·출력, 거부 조건, 의도된 차이와 이유를 정한다. 이 intent는 기존 커밋·PR 보고에 남기며 별도 장부를 만들지 않는다. 우연한 로그·예외 문자열은 계약으로 승격하지 않고 실제 소비자가 의존하는 출력·exit code는 보존한다.

실행 제어·파싱·정책·테스트 등록마다 Rust와 TS 중 책임자를 정한다. 같은 정책·목록·상태를 양쪽에 구현하지 않는다. 이미 있는 Cargo/Bun 명령·라이브러리를 재사용하고, 연결이 필요하면 명시적인 CLI 인자와 결과 형식으로 제한한다. 별도 범용 IPC·scheduler·process manager를 만들지 않는다. 개발용 Bun 하네스를 제품 서버의 JS fallback으로 넣지 않는다.

프로세스 경계가 바뀌면 argv, cwd, 필요한 env, stdin/stdout/stderr, spawn 실패, exit code와 signal, 취소·timeout, 직접 자식과 손자 처리 범위를 대조한다. 종료·출력 수집을 실제로 기다리고 자식을 회수하며, 동기 대기로 이벤트 루프의 취소·출력 처리를 막지 않는다. 정리 오류는 원래 실패를 덮거나 성공으로 바꾸지 않고 함께 보고한다. 기존 실행 한도와 자원 소유권을 보존하고 특정 signal이나 제한값을 모든 도구에 복제하지 않는다.

한 실행 경계씩 새 구현과 실제 호출자를 연결해 `fvoci-fast-verify`로 확인한 뒤 대체된 파일·wrapper를 제거한다. 의도적으로 나누는 이전은 남은 호출자와 완료 조건을 PR에 표시하고 두 정상 경로를 영구 유지하지 않는다. 독립 fixture·oracle는 역할과 호출자를 확인하며, 옛 언어라는 이유만으로 먼저 삭제하지 않는다.

## 손대지 말 것

- 서버 측 제품 연산의 기준은 Rust다. 새 Node 의존, 숨은 JS fallback, 내장 JS 엔진, JS 런타임 번들, 외부 변환 서비스 우회를 넣지 않는다. Vue/Tiptap, 브라우저 JS, 개발용 Node/CodeGraph, TS oracle, 합의한 PostgreSQL·Meilisearch·S3·SMTP는 제품 서버와 별개다.
- Bun에서 Bun/TS를 다시 띄울 때의 `process.execPath`와 Rust native helper·child는 AGENTS.md다. rlimit·env_clear를 파일시스템·네트워크 sandbox라고 하지 않는다. timeout과 Drop만으로 정리 완료라고 하지 않는다. `spawn_blocking`만으로 process 격리를 대체하지 않는다.
- parser·CRDT process 격리를 유지한다. 위험한 파싱을 HTTP 프로세스 안에 넣지 않는다. 편집 중 문서를 JSON 왕복으로 다시 만들어 삭제·동시편집 이력을 버리지 않는다.
- 하네스·Python 경계는 AGENTS.md 손대지 말 것이다.
- 새 검증 wrapper를 만들지 않는다. 손 절차를 대체하는 예외는 지정 task의 xtask 하위 명령뿐이다.
- Docker/build 입력, 설치 CI, manifest/lockfile은 그 경로의 작성자가 정해진 뒤에만 맞춘다. 개발·비교 fixture의 JS를 무조건 지우지 않는다.
- migrate의 current_exe를 서버로 보지 않는다. PATH에서 node만 숨기고 번들 runtime을 남긴 것을 Node 제거로 세지 않는다. 운영 배포·서비스 추가·권한 확대는 이 스킬의 범위가 아니다.
