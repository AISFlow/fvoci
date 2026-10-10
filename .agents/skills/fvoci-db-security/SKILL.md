---
name: fvoci-db-security
description: "인증, 인가, 세션, DB 쿼리, 트랜잭션, migration, 검색·공유·첨부 권한을 바꾸거나 검토할 때 쓴다. 실제 앱 역할과 DB 불변식을 완료 조건으로 두며, 순수 시각 조정에는 쓰지 않는다."
---

# 인가와 DB

## 완료 조건

- 바꾼 인가·저장이 실제 앱 DB 역할에서 성공과 거부·유실·경합을 모두 보여 주고, 그 검사가 exit 0이다. `TEST_DATABASE_URL`이 없으면 그 검사는 실패로 남긴다.
- 보고에 불변식별 성공·실패·미검증, DB 버전, 역할, 명령, SHA가 있다. 시크릿은 가린다.
- migration이 바뀌면 새 schema 설치와 기존 데이터 upgrade를 각각 검사한다. 쿼리·인덱스 성능이 목표면 `fvoci-postgres-performance`의 완료 조건도 맞다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

운영 DB가 아니라 소유권이 분리된 테스트 DB를 쓴다. 사용자 활성, 실제 멤버십, 리소스 권한, PAT 범위가 어디서 강제되는지 적는다. tenant id와 인증된 사용자 타입만으로 현재 권한을 증명하지 않는다.

권한 확인 뒤에 정지·탈퇴·철회가 먼저 끝나는 순서를 barrier 또는 DB 상태로 재현한다. 필요한 행 잠금, UPDATE 조건, 같은 트랜잭션 안 재확인을 검사한다. 본문·이벤트·감사의 commit과 rollback을 실패 주입으로 본다. 멱등성과 응답 유실 후 재시도의 의미를 확인한다.

RLS는 superuser가 아닌 앱 역할에서 본다. 연결 풀 재사용 전후에 tenant 컨텍스트가 남아 있지 않은지, 서로 다른 사용자·테넌트의 순차·동시 요청이 서로의 컨텍스트를 쓰지 않는지 본다. 마지막 관리자, 첫 설치, 세션 만료·폐기, 로그아웃·동의는 그 변경 범위에서 확인한다. 검색·캐시·worker·공유·export·collab에 같은 인가가 연결되는지도 본다.

OIDC/JWT·TOTP를 구현·교체하면 `fvoci-standard-implementations`를 함께 쓴다. SDK의 토큰 검증은 세션·identity·RLS·replay·원자성을 대신하지 않는다.

스키마는 `migrations`와 실제 query·caller의 PK·FK·UNIQUE·CHECK·NULL, 삭제·cascade, tenant 경계를 먼저 본다. 기존 데이터의 충돌·결측·backfill과 실행 중 잠금·실패 재개를 검토한다. 현재 runner의 트랜잭션·순서와 설치·업그레이드 제약을 따른다.

## 손대지 말 것

- mock, 단일 rollback fixture, 메모리 안전만으로 commit·복수 연결·RLS 경합을 통과시키지 않는다.
- 강제 실패 경로를 비보호 제품 endpoint로 열지 않는다.
- 이미 적용된 migration을 덮어쓰거나 자동 destructive reset으로 통과시키지 않는다. 되돌릴 수 없는 변환을 rollback 가능하다고 적지 않는다.
- PostgreSQL RLS·잠금·SQL이 SQLite·원격 libSQL에서도 같다고 가정하지 않는다. 엔진별 수락은 별도이고, 이 스킬이 지원 완료를 선언하지 않는다.
- 심각한 인가·데이터 결함은 그 기능 수락을 막는다. 상관없는 전체 앱 검증을 했다고 넓히지 않는다.
