---
name: fvoci-rust-slice
description: "Rust 백엔드 기능이나 서버 구조를 UI·API까지 연결할 때 쓴다. 고정 스택과 실제 호출 결과를 완료 조건으로 두며, Vue만의 기능·스타일 수정에는 쓰지 않는다."
---

# Rust 수직 구현

## 완료 조건

- 바꾼 기능이 실제 HTTP 또는 Vue에서 호출되고, 성공·거부·경합·외부 실패 중 해당 검사가 exit 0이다.
- 외부 계약(상태, 오류 코드, JSON, ID)이 유지된다. 컴파일과 health endpoint만으로는 완료가 아니다.
- DB·인가가 바뀌면 `fvoci-db-security`의 완료 조건도 맞다. 검사 명령은 `fvoci-fast-verify`다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

기준 SHA, 허용 파일, 호출 경로를 고정한다. 기존 회귀가 있으면 그 실패를 먼저 확인한다. 환경 준비 실패와 제품 실패를 구분한다.

입력·오류 타입, 순수 정책, 제품 연산, DB·외부 경계를 최소로 잇는다. 네트워크 입력은 따로 검증한다. enum, newtype, Result를 쓰고 문자열·Value·공유 Mutex로 타입을 덮지 않는다. 요청·작업·트랜잭션의 소유자와 종료 경로를 둔다.

구현을 더하기 전에 `fvoci-standard-implementations`의 선택 기준을 본다. 프로토콜·파서·SDK는 그 스킬, Node·child·build 경계는 `fvoci-runtime-boundaries`다. 해당 없는 작업에서 후보 전체를 조사하지 않는다.

외부 오류는 `src/error.rs`의 AppError·ProblemCode에 연결한다. 내부 원인은 tracing에 남기고 시크릿은 남기지 않는다. `src/db/mod.rs`의 pool은 clone 가능한 공유 handle이다. async 작업은 취소·timeout 뒤 DB·외부 효과와 자원 해제를 확인한다. CPU·blocking 작업은 기존 격리와 bounded 실행을 따른다.

## 손대지 말 것

- 서버 스택은 Rust stable, Tokio, axum 0.8, Tower/tower-http, SQLx/PostgreSQL, Serde, tracing, Yrs, rhwp다. 실제 구현을 막는 검증된 문제가 없으면 웹 프레임워크를 교체하지 않는다.
- 이번 차수에 Yjs/Yrs 교체를 넣지 않는다. Yrs 유지를 미리 결론내지 않고, 미해결 결함을 비교의 새 선행 조건으로 만들지 않는다.
- 범용 repository, DI, 불필요한 trait, 범용 실행 프레임워크를 먼저 만들지 않는다. `Arc<Mutex<PgPool>>`로 요청을 직렬화하지 않는다. `spawn_blocking`만으로 process 격리를 대체하지 않는다.
- 기존 JS 서버 위임, 테스트용 인증 우회, 성공 하드코딩을 제품 경로에 넣지 않는다.
- 원본의 버그를 구현 복제의 정답으로 두지 않는다. JS 위임을 이식 완료로 세지 않는다.
