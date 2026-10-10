---
name: fvoci-postgres-performance
description: "PostgreSQL 쿼리, 인덱스, 페이지네이션, N+1, 잠금, 풀이 느릴 때 쓴다. 측정한 계획과 지연을 완료 조건으로 두며, 스키마 설계나 다른 DB 지원 설계를 대신하지 않는다."
---

# PostgreSQL 성능

## 완료 조건

- 같은 조건에서 변경 전후 쿼리 수, 계획, 행 수, 지연, 쓰기 비용이 적혀 있고, 관련 앱 역할의 인가·철회·commit·rollback·경합 검사가 exit 0이다.
- 개선되지 않았거나 재지 않은 값도 결과에 남는다. 시크릿과 실사용자 데이터는 보고서에 없다.

## 기본 절차

대상은 현재 SQLx/PostgreSQL이다. 호출자, SQL, bind 분포, 반환 행, 왕복, DB 버전, 앱 역할·RLS, 데이터 규모, 동시성, cache warm/cold를 고정한다. endpoint 지연을 pool acquire, lock wait, DB 실행, 전송·직렬화로 나눈다.

소유한 테스트 DB에서 `EXPLAIN`으로 계획을 본다. 필요할 때만 `EXPLAIN (ANALYZE, BUFFERS)`로 실제 행·loops·filter·sort spill·buffer와 추정 오차를 본다. ANALYZE는 쿼리를 실행한다. 쓰기나 부작용 함수는 폐기 가능한 fixture에서만 실행한다. rollback이 sequence와 외부 부작용까지 되돌린다고 가정하지 않는다.

인덱스는 실제 WHERE/JOIN/ORDER BY의 equality·range·정렬에 맞춘다. composite 선두 열, partial predicate, covering의 크기·쓰기 비용, 기존 index와의 중복, FK 참조 키와 참조측 index를 구분한다. CONCURRENTLY는 migration runner의 트랜잭션·복구를 먼저 확인한다.

N+1은 루프 안 호출을 재고 필요한 열만 batch/join한다. 결과 중복·정렬·메모리·항목별 인가를 유지한다. 깊은 OFFSET이 병목이면 안정된 정렬과 고유 tie-breaker가 있는 keyset을 검토한다. NULL, 방향, 동시 삽입·삭제, 필터, 권한, 기존 cursor 의미를 검사한다.

잠금은 `src/db/context.rs`의 transaction-local `set_config(..., true)`, credential 재검사, 모듈별 잠금 순서를 따른다. `src/collab/guard.rs`의 전용 연결과 session advisory lock은 transaction pooling과 따로 본다.

인가·원자성은 `fvoci-db-security`, 검사 선택은 `fvoci-fast-verify`다. 계획 해석은 실제 서버 버전의 [EXPLAIN](https://www.postgresql.org/docs/18/using-explain.html)과 대조한다. 검토 참고는 [Supabase agent-skills 고정 revision](https://github.com/supabase/agent-skills/tree/544bfc56c89afe2b87b20017a59b2c6e9502a1fb) (MIT)이며, `auth.uid()`, SECURITY DEFINER 우회, session SET·pooler recipe는 이식하지 않는다.

## 손대지 말 것

- 작은 fixture의 sequential scan이나 planner cost만으로 개선을 선언하지 않는다.
- 연결을 무한 병렬화하거나 pool 크기를 키워 부하를 옮기지 않는다. 무제한 fetch 후 앱에서 자르거나, 페이지마다 비싼 count를 더하지 않는다.
- 철회 경합을 막는 잠금을 성능 이유로 빼지 않는다. 일반 pooler·session SET 조언으로 room guard를 바꾸지 않는다.
- SQLite·원격 libSQL을 현재 검증된 지원으로 표시하지 않는다. 이 스킬을 적용하려고 범용 DB 추상화나 다른 엔진 migration을 먼저 만들지 않는다.
- 운영 DB 변경이나 설정 튜닝 권한을 성능 조사에서 추론하지 않는다.
