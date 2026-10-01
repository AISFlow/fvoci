---
name: fvoci-runtime-boundaries
description: Node 서버 경로 대체, 제품 설치 준비, Cargo crate·바이너리·운영 명령·native child·배포/build 경계 변경에 사용한다. 단순 Vue 기능·스타일이나 경계에 영향 없는 문서 수정에는 사용하지 않는다.
---

# Rust 런타임·설치·native child·빌드 경계

공통 제품·승인 범위는 루트 AGENTS.md, 구체적인 런타임·설치 계약은 이 스킬이 정본이다. 전체 재설계나 무조건 단일 바이너리가
목표는 아니다. 진행 중 변경을 보존하고 해당 경계의 안전성·개발 비용·배포 비용을 함께 줄인다.

## 입력

고정 SHA와 실제 호출자, 제품/개발용 의존성, Cargo target/feature/lockfile, 배포 입력과 실행 설정,
child·DB 연결 등 소유 자원, 기존 취소·종료·복구 회귀. 미완료 기능 목록은 진행 기록에서 읽는다.

## 절차

1. **경계를 구분한다.** crate/컴파일 단위, 배포 실행 파일, 실제 process, 독립 service, 운영자 명령은
   서로 다르다. 현재 build 명령·최종 이미지·호출 경로에서 해당 변경의 범위만 찾는다. 그래프에 안 보이는
   환경 변수·Command·IPC·SQL 호출은 원문으로 보완한다. 전체 저장소 목록을 새 문서에 복제하지 않는다.
2. **분리 이유를 확인한다.** 기존 library/helper 재사용, 같은 실행 파일의 내부 모드, 별도 helper를
   비교한다. crate가 늘었다고 새 서비스가 필요한 것은 아니다. 무거운 parser/Yrs를 한 binary에 합쳐
   일반 API 변경의 링크 비용을 늘리거나, 파일 수를 줄이려고 HTTP process 안에서 위험한 파싱을 하지 않는다.
3. **권한과 수명주기를 유지한다.** 기본 설치 준비(설정 검증·준비 확인·migration·grant·검색 키)는
   앱 컨테이너의 시작 절차가 수행하고, 준비가 성공한 뒤에만 제한된 앱 역할로 정상 서버를 시작한다.
   정상 서버 프로세스에는 소유자 credential·master key를 남기지 않으며, 다른 쓰기 서버가 살아 있는
   동안의 schema 변경과 안전 조건을 확인할 수 없는 업그레이드는 거부한다. env 정리나 SET ROLE만으로
   경계를 주장하지 않고 실제 uid·파일 권한·환경·FD를 확인한다. exec 환경 필터는 컨테이너 설정의 값을
   지우지 않으므로 `docker inspect`·`docker exec`에 설정값이 없다고 주장하지 않는다. 같은 실행 파일의
   child 모드는 서버 초기화·credential 로딩·listen 전에 분기하고 설치 절차를 실행하지 않는다. 자식에는 필요한 환경만 주고 입력/출력·시간·메모리·동시 실행·취소·종료/회수를 검증한다.
   rlimit·env_clear는 파일시스템/네트워크 sandbox가 아니며 timeout/Drop만으로 정리 완료라고 하지 않는다.
4. **Node 경로를 단계적으로 바꾼다.** 남아 있는 실제 변환 호출과 의존하는 가져오기·내보내기·법률·
   공유 경로를 찾는다. [표준 구현 스킬](../fvoci-standard-implementations/SKILL.md)로 적합한 Rust 구현을
   먼저 확인한다. 기존 TS는 격리된 개발용 oracle로 쓰고, 검증된 단위부터 호출을 바꾸며 동작하는 기능을
   먼저 삭제하지 않는다. 새 framework·숨은 JS fallback·외부 변환 서비스로 대체하지 않는다.
5. **제품 의미로 비교한다.** Markdown/Tiptap·HTML·문서 출력의 구조·표·서식·한글/emoji·이미지·링크·
   인가·오류·한도를 보존한다. HTML은 sanitize/URL/CSP도 검사한다. 파일이 열리는 것만으로 충분하지 않고
   비결정적 메타데이터의 binary 차이만으로 실패시키지도 않는다. 신규 가져오기의 CRDT 생성과 기존 문서
   이력 보존을 구분하며, 편집 중 문서를 JSON 왕복으로 재생성해 삭제·동시편집 이력을 버리지 않는다.
6. **배포와 복구까지 전환한다.** 확인된 경로의 기존 runtime·패키지·환경 변수·스크립트를 제거하고,
   Docker/build 입력·설치 CI 경로 필터·manifest/lockfile은 AGENTS.md의 소유권 조정을 받은 뒤에만 맞춘다. 개발/비교 fixture의 JS까지 무조건 삭제하지 않는다.
   새 helper·객체 종류가 생기면 설치·취소·재시작·저장소/암호화 키 복구 확인에도 연결한다.
   옛 converter 어댑터 제거는 변환 기능·진단 제거가 아니다. doctor는 실제 제품 실행 파일의 Rust child를
   작은 입력으로 점검하며 실행 파일·글꼴·한도 오류를 실패로 보고한다. migrate의 current_exe를 서버로 간주하지 않는다.
   개발·CI·fixture·독립 reader의 Python은 언어만을 이유로 재작성하지 않는다. 운영 Python은 실제 호스트·
   관리/제품 컨테이너 중 실행 위치와 준비 검사를 명시한다. 대체 시 검사 독립성·부정 검사·복구 책임을 보존한다.
   백업 manifest·필수 키 호환성 같은 제품 운영 판단은 기존 Rust 키 정책과 공유하고, shell은 Compose·PG·
   저장소 작업만 연결한다. 독립 Python 호환성 fixture는 운영 판단 경로와 분리해 보존할 수 있다.

## 비용과 최종 검증

의사결정에 필요한 일반 API 수정의 warm build, clean 제품 build, 최종 산출물 크기, child 시작/IPC·
메모리·종료 비용을 같은 조건에서 비교한다. binary 수만으로 중복 컴파일을 단정하지 않고 target·
feature·의존성 재사용을 본다. 측정하지 않은 성능 개선을 주장하거나 새 benchmark 플랫폼을 만들지 않는다.

전체 Node 제거 수락은 Node/Bun/Deno·내장 JS 엔진이 없는 최종 제품 환경에서 실제 변환·공유·설치·
복구 경로를 실행한 근거가 필요하다. PATH에서 node만 숨기고 번들 runtime을 남긴 것은 제거가 아니다.
개발 호스트의 Node/CodeGraph, 브라우저 JS, 합의한 외부 시스템은 제품 서버 런타임과 구분한다.
PDF 헤더·ZIP/XML·문자열 검사는 출력 품질 전체를 증명하지 않는다. 기존 소비자·독립 reader/renderer 근거를
재사용하고 미실행 시각 품질 범위를 남긴다. 내용·서식 축소는 ‘의도적 차이’ 표기만으로 수락하지 않는다.
필요한 DB·협업 검사는 [fast-verify](../fvoci-fast-verify/SKILL.md)와
[db-security](../fvoci-db-security/SKILL.md)를 사용하고 관련 없는 전체 검증을 반복하지 않는다.

## 결과

기존 PR/task에 유지/제거한 실행 경계와 이유, 실제 호출 전환·검증 SHA, 측정 조건, 남은 runtime 의존성과
복구 범위를 짧게 남긴다. 하나의 명령·한 파일·한 process를 만드는 것 자체가 수락 조건은 아니다.
운영 배포·서비스 추가·권한 확대는 이 스킬의 승인 범위가 아니다.

## 제품 런타임과 설치의 필수 계약

서버 측 제품 연산은 Rust가 기준이며 남아 있는 Node 변환 경로는 영구 예외가 아니라 잔여 포팅이다.
새 Node 의존은 늘리지 않는다. 최종 제품에 Node/Bun/Deno 서버 기능·JS worker 위임·내장 JS 엔진·
JS 런타임 번들·외부 변환 서비스로의 우회를 남기지 않는다. Vue/Tiptap·브라우저 JS·개발용
Node/CodeGraph·TS 비교 oracle와 합의한 PostgreSQL·Meilisearch·S3·SMTP는 별개다.
기본 설치 준비(설정 검증, DB·검색 준비 확인, 앱 역할·migration·grant·검색 키)는 메인 앱 컨테이너의
시작 절차가 자동 수행한다(2026-09-28 사용자 결정). 정상 요청 처리 단계는 제한된 앱 DB 역할과 필요한
설정만으로 실행하며, 준비 단계의 소유자 credential·Meili master key를 정상 서버 프로세스에 남기지
않는다. 준비 실패·키 누락·안전하지 않은 업그레이드(다른 쓰기 서버가 살아 있는 상태의 schema 변경,
미지원 rolling upgrade, DB downgrade)는 거부한다. 필요한 parser·CRDT process 격리를 유지한다.
같은 실행 파일을 child로 쓰면 내부 모드는 서버 초기화·credential 로딩·listen 전에 분기한다.
전체 Node 제거 수락은 Node/Bun/Deno·내장 JS 엔진이 없는 최종 제품 환경에서 실제 경로를 실행한
근거가 필요하다.
