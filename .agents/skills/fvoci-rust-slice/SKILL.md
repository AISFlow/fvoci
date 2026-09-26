---
name: fvoci-rust-slice
description: FVOCI Rust 백엔드 기능을 HTTP 입력부터 영속화와 응답까지 구현하거나 기존 프론트엔드에 연결할 때 사용한다.
---

# 작은 수직 기능 구현

공통 운영 규칙은 루트 AGENTS.md, 기능 기준은 현재 task와 원본 계약을 따른다.

## 입력

기준 SHA, 허용 파일, source-contract 결과, 실제 호출 경로, 필요한 검증 명령과 수락 조건.

표준 프로토콜·파서·SDK를 만들거나 바꾸면 [standard-implementations](../fvoci-standard-implementations/SKILL.md),
Node 대체·바이너리·child/build 경계를 바꾸면 [runtime-boundaries](../fvoci-runtime-boundaries/SKILL.md)를 먼저 적용한다.
해당하지 않는 작업에서 후보 라이브러리 전체를 조사하지 않는다.

## 절차

1. 성공/거부/경합·외부 실패 중 해당 기능의 핵심 재현 검사를 먼저 정한다. 기존 회귀가 있으면 실제 실패를 확인한다. 실패 원인이 환경 준비인지 제품인지 구분한다.
2. 입력/오류 타입, 필요한 순수 정책, 제품 연산과 DB/외부 경계를 최소로 연결한다. 범용 repository·DI·불필요한 trait/crate를 먼저 만들지 않는다.
3. 네트워크 입력은 별도로 검증한다. enum/newtype/Result를 의미 있게 쓰고 문자열·Value·공유 Mutex로 타입 문제를 덮지 않는다. 요청·작업·트랜잭션의 소유자와 종료 경로를 둔다.
4. DB/인가 변경이면 fvoci-db-security를 함께 적용한다. 컴파일 가능한 Rust와 동일한 DB 의미를 혼동하지 않는다.
5. HTTP 또는 실제 프론트엔드에서 호출해 결과를 확인한다. 기존 JS 서버 위임·테스트용 인증 우회·성공 하드코딩을 제품 경로에 넣지 않는다.
6. 허용 경로만 정리하고 fvoci-fast-verify로 관련 검사를 실행한 뒤 fvoci-handoff 형식으로 제출한다. 공통 manifest/lock/migration이 필요하면 먼저 단독 소유권을 받는다.

## 완료 조건

실제 호출 가능한 기능, 보존된 외부 계약, 해당 실패 검사와 실제 실행 결과가 있어야 한다. 뼈대·health endpoint·컴파일 성공만으로 기능 완료를 선언하지 않는다.
