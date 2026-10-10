# FVOCI

Rust 서버와 Vue 3 웹으로 다시 쓰는 FVOCI입니다. 서버 런타임은 Rust(Tokio, axum, SQLx)이고, 데이터 범위는 PostgreSQL·로컬 SQLite·원격 libSQL/Turso입니다. UI는 `apps/web`의 Vue 3 + Nuxt UI + Vite이며 `index.html`이 `/src/vue/entry.ts`를 띄웁니다. `compat/`의 JS는 협업 프로토콜 조사용이며 제품 서버가 호출하지 않습니다. 무엇이 수락됐는지는 [docs/rewrite.md](docs/rewrite.md)가 범위와 수락 단위를 담습니다.

실행 방법과 환경 변수는 [RUNNING.md](RUNNING.md)를 참조하세요.

```sh
cargo fetch --locked
cargo fmt --check
cargo check --locked --offline --all-targets --features db-tests
cargo clippy --locked --offline --all-targets --features db-tests -- -D warnings
cargo test --locked --offline --lib
```

빠른 검사는 외부 DB나 Docker를 요구하지 않습니다. 실제 인가·원자성·경합 검사는 별도 PostgreSQL 환경에서 `cargo test --locked --offline --features db-tests --test db_integration`으로 실행하며 `TEST_DATABASE_URL`이 없으면 실패합니다. 지원이 끝났는지는 [docs/rewrite.md](docs/rewrite.md)의 수락 단위로 판단합니다.

에이전트 운영 규칙은 [AGENTS.md](AGENTS.md)에 있습니다. 라이선스는 [MIT](LICENSE)입니다.
