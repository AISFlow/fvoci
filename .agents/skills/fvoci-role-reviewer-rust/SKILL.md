---
name: fvoci-role-reviewer-rust
description: "사람이 Rust 리뷰어 역할을 명시해 부를 때만 쓴다. DB가 아닌 Rust, xtask, 설치·probe shell, vendor 바이트를 판정한다."
disable-model-invocation: true
---

## 완료 조건

- 범위는 DB가 아닌 Rust, `xtask`, 설치·probe shell, `vendor/` 바이트 비교다. `src/db/**`는 `fvoci-role-reviewer-db`다. 주 리뷰어·교차·양쪽 트리·결과 이름은 AGENTS.md다.
- 출력에 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING이 있다.
- `cargo fmt --check`와 `cargo clippy --all-targets -- -D warnings`가 exit 0이다. `xtask` 변경은 depth 1 checkout에서 `cargo test`가 루트와 `xtask/` 둘 다 PASS다.
- 의존성 변경에는 이유, 라이선스, lock diff가 있고 `cargo tree -d`에 새 중복이 없다. Python→TS/Rust 이전은 이 리뷰어가 만든 반례로 본다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다. diff와 호출자만 보고, blocking이면 REQUEST_CHANGES다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 작성자와 다른 주체다. AGENTS.md. 워크플로·웹은 `fvoci-role-reviewer-ci-web`이다.

## 출력

```
판정: REQUEST_CHANGES
SHA: cccccccccccccccccccccccccccccccccccccccc  parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
patch-sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking: src/api/documents.rs 빈 결과가 404다. 계약은 200과 빈 배열.
검사: cargo fmt --check PASS. cargo clippy --all-targets -- -D warnings PASS. vendor 바이트 커밋 트리 PASS, main 병합 결과 NOTRUN.
```
