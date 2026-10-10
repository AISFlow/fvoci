---
name: fvoci-standard-implementations
description: "RFC, 프로토콜, 토큰, 파서, 직렬화, 암호, SDK를 고르거나 바꾸기 전에 쓴다. 재사용과 얇은 정책 연결을 완료 조건으로 두며, 일반 CRUD·문서 수정에는 후보 비교를 하지 않는다."
---

# 표준 구현 선택

## 완료 조건

- 추가·교체 결과에 적용 명세, 선정 버전·feature, 맡긴 책임, 남긴 제품 정책, 제거한 자체 구현, 검증 SHA, 미검증 범위가 있다.
- 공식 벡터와 허용·거부 입력, 실제 client/server, 현재 DB·UI·복구 경계의 검사가 exit 0이다.
- 일반 기능은 아래 선택 기준만 만족하면 된다. 명세·후보 비교는 표준 경계가 바뀔 때만 한다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

선택 기준: 새 상태·추상화·의존성 전에, 기존 코드 → 표준 라이브러리·플랫폼 → 이미 설치된 의존성 순으로 필요한 보장을 만족하는지 본다. 요구를 줄이거나, 표준·보안 처리를 짧은 자체 코드로 바꾸지 않는다. 단순화할 함수의 흐름과 호출자를 먼저 본다. 오류는 증상이 아니라 원인이 있는 경계를 고친다. 공통화 전에 호출자별 정책 차이를 확인한다. 삭제의 이득은 줄 수가 아니라 직접 소유할 상태·정책·의존성이 줄어드는지로 본다. 이 기준은 [Ponytail의 선택 원칙](https://github.com/DietrichGebert/ponytail/blob/e3ba2aa6f1e6f0bc4d69eb09c9f0d0a93af56156/skills/ponytail/SKILL.md)을 참고해 FVOCI에 맞춰 새로 썼다([MIT](https://github.com/DietrichGebert/ponytail/blob/e3ba2aa6f1e6f0bc4d69eb09c9f0d0a93af56156/LICENSE)). 외부 스킬의 전문, 지속 모드, 출력 제한, 최단 diff, 검사 수 제한은 가져오지 않는다.

표준 경계일 때: 명세 버전, 필수·선택, errata, 보안 BCP, 실제 클라이언트 범위를 고정한다. 모든 RFC의 강제력이 같다고 보지 않는다. 현재 의존성 → 공식 SDK·유지보수되는 Rust 구현 → 얇은 adapter 순으로 비교한다. 암호 primitive만 재사용하고 JWT/OIDC 전체를 위임했다고 하지 않는다. `references/candidates.md`는 관련 절만 읽는 시작점이다. tar header·entry 검사는 유지보수되는 parser에 맡기고, 경로·타입 허용만 제품에 둔다.

라이브러리는 파싱·검증·직렬화·프로토콜 수명주기를, FVOCI는 계정 연결·현재 권한·세션 철회·DB 원자성·저장·재시도 정책을 맡는다. 공식 출처, 버전, 라이선스, 유지보수, toolchain·Tokio/HTTP/TLS, 최소 feature, 전이 의존성, redirect·proxy·외부 schema, 입력·시간·메모리 한도, 취소·종료를 확인한다. manifest/lockfile 담당을 받은 뒤 배포 버전을 lockfile로 고정한다. 한 경계만 교체한다. 검증된 구간의 자체 parser와 중복 정상 경로는 제거한다.

## 손대지 말 것

- 무관한 CRUD에 후보 전체를 조사하지 않는다. 기존 차단 수정과 담당 작업을 보존한다.
- 보안 검증 실패를 느슨한 구현으로 재시도하는 fallback을 두지 않는다. 검증 전 입력을 검증된 identity와 섞지 않는다.
- issuer/subject 매핑, replay 방지, lease/fence, RLS, 영속 commit, 복구 키 보관은 라이브러리가 자동으로 채우지 않는다. 손으로 쓴 비교 루프의 timing을 컴파일러와 무관하게 보장한다고 적지 않는다.
- README의 준수 주장, 인기, Rust 사용, 권고 부재를 감사 완료로 해석하지 않는다. 무관한 upgrade나 moving-main 의존성을 함께 넣지 않는다.
- 라이브러리를 쓰려고 새 서비스·바이너리·범용 framework를 만들지 않는다. 별도 평가 플랫폼, 중복 backlog, 프로토콜 버전 자동 업그레이드를 만들지 않는다.
- 긴 교체가 긴급 보안 수정을 막으면 최소 수정부터 수락하고 교체는 별도 PR로 둔다.
