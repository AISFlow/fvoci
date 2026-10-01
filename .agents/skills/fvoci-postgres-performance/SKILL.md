---
name: fvoci-postgres-performance
description: FVOCI PostgreSQL 쿼리·인덱스·페이지네이션·N+1·잠금 대기·풀 병목의 측정과 개선에 사용한다. DB 보안은 fvoci-db-security를 함께 읽으며 일반 스키마 설계나 미래 DB 지원 설계를 대신하지 않는다.
---

# PostgreSQL 실행계획·병목 측정과 안전한 개선

공통 운영은 [AGENTS.md](../../../AGENTS.md), 인가·원자성 수락은
[db-security](../fvoci-db-security/SKILL.md)를 따른다. 현재 SQLx/PostgreSQL 구현을 대상으로 한다.
SQLite·원격 libSQL 지원은 향후 목표이며 현재 구현·검증된 지원으로 표시하지 않는다.
이 스킬을 적용하기 위해 범용 DB 추상화나 다른 엔진 migration을 먼저 만들지 않는다.

## 측정 먼저

1. 실제 호출자·SQL·bind 분포·반환 행 수·왕복 횟수와 느린 조건을 고정한다. 사용 DB 버전,
   앱 역할/RLS 컨텍스트, 데이터 규모·tenant 편중, 동시성, cache warm/cold 조건을 기록한다.
   endpoint 지연을 pool acquire·lock wait·DB 실행·전송/직렬화로 나눠 병목을 찾는다.
2. 대표 fixture의 소유한 테스트 DB에서 먼저 `EXPLAIN`으로 계획을 본다. 필요할 때만
   `EXPLAIN (ANALYZE, BUFFERS)`로 실제 행 수·loops·filter 제거·sort spill·buffer 비용과
   추정 오차를 확인한다. ANALYZE는 쿼리를 실행한다. 쓰기나 부작용 있는 함수는 폐기 가능한
   fixture에서만 실행하며 rollback이 sequence·외부 부작용까지 되돌린다고 가정하지 않는다.
3. query count/실행계획과 종단 지연을 변경 전후 같은 조건에서 비교한다. 작은 fixture의
   sequential scan은 정상일 수 있다. index 사용 여부나 planner cost만으로 개선을 선언하지 않는다.
   통계·selectivity·대표 parameter 차이도 확인하고 시크릿/실사용자 데이터는 보고서에 넣지 않는다.

## 병목별 최소 변경

- 인덱스: 실제 WHERE/JOIN/ORDER BY의 equality·range·정렬 순서에 맞춰 후보를 고른다.
  composite의 선두 열, partial predicate의 실제 query 일치, covering의 크기·쓰기 비용을 본다.
  기존 index/constraint와 중복되는지 확인하며 FK가 참조하는 키의 index와 참조측 index를 구분한다.
  추가/삭제·CONCURRENTLY는 기존 migration runner의 transaction·실패 복구 제약을 먼저 확인한다.
- N+1: 루프 안 DB 호출을 측정하고 필요한 열만 batch/join으로 가져온다. 결과 중복·정렬·메모리,
  항목별 인가를 보존한다. 연결을 무한 병렬화하거나 SQLx pool 크기를 키워 부하를 옮기지 않는다.
- 페이지네이션: 깊은 OFFSET 비용이 실제 병목이면 안정된 전체 정렬과 고유 tie-breaker를 가진
  keyset을 검토한다. NULL·오름/내림·동시 삽입/삭제·필터·권한을 검사하고 기존 cursor/API
  의미를 보존한다. 무제한 fetch 후 앱에서 자르거나 페이지마다 비싼 count를 추가하지 않는다.
- 잠금/풀: [context](../../../src/db/context.rs)의 transaction-local `set_config(..., true)`,
  credential 재검사와 모듈별 잠금 순서를 확인한다. read/write를 이름만으로 분류하지 말고
  실제 snapshot/row lock 계약을 따른다. 철회 경합을 막는 잠금을 성능 이유로 제거하지 않는다.
  [협업 room guard](../../../src/collab/guard.rs)의 전용 연결·session advisory lock은 transaction
  pooling과 별도 검토가 필요하다. 일반 pooler/session SET 조언으로 바꾸지 않는다.

## 수락 보고

관련 실제 앱 역할에서 인가·철회·commit/rollback·경합 회귀를 재실행하고, 변경 전후 계획·행 수·
쿼리 수·지연·쓰기 비용과 조건을 함께 제출한다. 개선되지 않았거나 미측정인 결과도 남긴다.
검사는 [fast-verify](../fvoci-fast-verify/SKILL.md), schema/migration의 안전성은 db-security를 따른다.
운영 DB 변경·설정 튜닝 권한을 성능 조사 요청에서 추론하지 않는다.

## 출처와 적용 범위

FVOCI에 맞춰 새로 작성했으며 외부 SQL recipe나 스킬 전문을 복사하지 않았다.
검토 참고: [Supabase agent-skills 고정 revision](https://github.com/supabase/agent-skills/tree/544bfc56c89afe2b87b20017a59b2c6e9502a1fb)
(MIT). Supabase `auth.uid()`, SECURITY DEFINER 우회, session SET·pooler recipe는 이 프로젝트에
이식하지 않는다. 계획 해석은 [PostgreSQL EXPLAIN](https://www.postgresql.org/docs/18/using-explain.html)을
실제 서버 버전과 대조한다. 외부 권고는 프로젝트 보안 계약을 대체하지 않는다.
