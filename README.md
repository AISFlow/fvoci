# FVOCI Rust backend

FVOCI 백엔드의 단계적 Rust 재작성입니다. 현재 PostgreSQL 기반 초기 설치·로그인·세션·인증된 사용자 프로필 변경을 구현하고 실제 HTTP·DB 검증을 마쳤습니다. 전체 FVOCI를 대체하는 릴리스가 아닙니다.

실행 방법과 환경 변수는 [RUNNING.md](RUNNING.md), 고정 원본 기준선·검증 결과·남은 기능은 [docs/rewrite.md](docs/rewrite.md)를 참조하세요.

```sh
cargo fetch --locked
cargo fmt --check
cargo check --locked --offline --all-targets --features db-tests
cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings
cargo test --locked --offline --lib
```

빠른 검사는 외부 DB나 Docker를 요구하지 않습니다. 실제 인가·원자성·경합 검사는 별도 PostgreSQL 환경에서 `cargo test --locked --offline --features db-tests --test db_integration`으로 실행하며 `TEST_DATABASE_URL`이 없으면 실패합니다.

서버 런타임은 Rust와 PostgreSQL을 사용합니다. `compat/`의 JS 도구는 협업 프로토콜 조사 전용이며 제품 서버에서 호출하지 않습니다. 기존 프론트엔드 연결, 협업 서버, 문서 처리, 데이터 이관·백업·복원 및 다른 DB 엔진 지원은 아직 완료되지 않았습니다.

에이전트 운영 규칙은 [AGENTS.md](AGENTS.md), 실제 도구 검증 기록은 [.agents/environment.md](.agents/environment.md)에 있습니다. 라이선스는 [MIT](LICENSE)입니다.
