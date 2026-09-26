# 표준 구현 후보와 제품 경계

관련 영역만 읽는다. 아래 링크는 공식 명세와 라이브러리 문서의 조사 시작점이며 의존성 고정 목록이
아니다. `latest` 문서는 설치 지시가 아니다. 채택할 때 실제 릴리스·API·feature·보안 권고·호환 버전을
다시 확인해 PR에 남긴다. 현재 채택 여부와 진행률은 이 자료가 아니라 제품 코드·lockfile·진행 기록이 정본이다.

## OAuth/OIDC·JWT

- 기준: [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html),
  [PKCE RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html),
  [OAuth 보안 BCP RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html),
  [JWT BCP RFC 8725](https://www.rfc-editor.org/rfc/rfc8725.html).
- 후보: [openidconnect](https://docs.rs/openidconnect/latest/openidconnect/)와 OAuth 전용 경로의
  [oauth2](https://docs.rs/oauth2/latest/oauth2/). 저수준 JWT 구현을 추가로 겹치기 전에 상위 SDK의 범위를 확인한다.
- 검사: 허용 알고리즘·서명·issuer/audience·nonce·state·PKCE·토큰 용도, 키 회전, 다른 issuer의 같은 sub.
  검증된 `(issuer, subject)`의 영속 연결과 provider 설정 변경 의미, link 최종 쓰기의 현재 세션 재검증은
  FVOCI 책임이다. 외부 응답 대기 중 철회를 barrier로 재현한다. 로컬 세션/PAT를 JWT로 바꾸라는 뜻은 아니다.
- 기본 HTTP client를 맹신하지 않는다. discovery/JWKS/token 등 각 접근의 DNS·주소 고정·redirect/proxy·
  시간/응답 한도를 확인한다. 공급자 차이를 지원하려고 공통 서명·issuer 검사를 끄지 않는다.

## MCP·JSON Schema

- 기준/후보: [MCP 명세](https://modelcontextprotocol.io/specification/),
  [공식 Rust SDK](https://github.com/modelcontextprotocol/rust-sdk), [rmcp](https://docs.rs/rmcp/latest/rmcp/).
  MCP는 IETF RFC가 아니다. 원본과 실제 client의 협상 버전·capability·transport부터 비교한다.
- 초기화·notification·요청 ID·잘못된 입력·취소·종료, stdio의 stdout 오염, 실제 tool/API 매핑·PAT 권한을
  검사한다. SDK 채택과 미구현 tool/API 또는 transport의 완성은 별개다. protocol error와 tool error를 구분한다.
- 고정 DTO는 Serde 검증으로 충분한지 먼저 본다. 일반 schema 검증이 필요할 때
  [jsonschema](https://docs.rs/jsonschema/latest/jsonschema/)를 비교한다. schema 생성과 검증은 다르다.
  draft·keyword·format·default/coercion·unknown field 의미를 고정하고 HTTP/file `$ref` 자동 조회를 막는다.

## TOTP·Base32

- 기준: [HOTP RFC 4226](https://www.rfc-editor.org/rfc/rfc4226.html),
  [TOTP RFC 6238](https://www.rfc-editor.org/rfc/rfc6238.html),
  [Base32 RFC 4648](https://www.rfc-editor.org/rfc/rfc4648.html).
  후보: [totp-rs](https://docs.rs/totp-rs/latest/totp_rs/). provisioning URI는 실제 authenticator와 별도 확인한다.
- 공식 벡터·시간 경계·허용 skew·기존 secret/digits/period를 보존한다. 반환된 검증 step의 재사용 방지,
  recovery code 단일 사용, challenge 소모와 세션 발급 원자성은 실제 DB에서 검사한다. QR 생성을 위해
  secret을 외부 서비스에 보내지 않는다. bool 성공만으로 DB replay 검사를 제거하지 않는다.

## 일정·HTTP·메일·저장소·문서

- ICS: [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html),
  [icalendar](https://docs.rs/icalendar/latest/icalendar/). UTF-8 줄 접기·escaping·종일 일정·시간대/DST와
  외부 달력 읽기를 확인한다. serializer가 반복 일정·쿼리·접근 정책까지 보장하거나 CalDAV가 필요하다고 보지 않는다.
- HTTP: [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html),
  [Content-Disposition RFC 6266](https://www.rfc-editor.org/rfc/rfc6266.html),
  [Problem Details RFC 9457](https://www.rfc-editor.org/rfc/rfc9457.html). 기존 axum/http/tower·reqwest/rustls를
  먼저 활용한다. Range·캐시·인가·오류 응답의 제품 계약은 별도이며 표준화를 이유로 일괄 변경하지 않는다.
- SMTP/MIME·S3는 기존 lettre·rusty-s3/ObjectStorage를 먼저 확인한다. 서명·문법을 다시 작성하거나
  단순히 더 큰 SDK로 교체하지 않는다. 전달/commit/재시도, presigned·multipart·삭제/복구 의미는 제품에서 확인한다.
- 문서/CRDT는 RFC 유무가 아니라 해당 형식·라이브러리·현재 client 계약이 기준이다. Yrs/rhwp와 적합한
  parser/writer를 재사용하되 Tiptap schema·원본 bytes·후속 편집·출력 품질은 직접 연결 검증한다.

## 의존성·실행 경계 참고

[RustSec](https://rustsec.org/)와 해당 프로젝트의 보안 공지에서 선정 버전을 확인한다. 권고가 없다는 것은
무결함 증명이 아니다. [Cargo target](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)의 library와
binary를 구분하고, 배포/process 변경에는 [runtime-boundaries](../../fvoci-runtime-boundaries/SKILL.md)를 적용한다.
