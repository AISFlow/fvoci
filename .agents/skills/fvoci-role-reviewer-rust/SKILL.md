---
name: fvoci-role-reviewer-rust
description: "사람이 Rust 리뷰어 역할을 명시해 부를 때만 쓴다. DB가 아닌 Rust, xtask, 설치·probe shell, vendor 바이트를 판정한다."
disable-model-invocation: true
---

# Rust 리뷰어

## 완료 조건

- 범위는 DB가 아닌 Rust, `xtask`, 설치·probe shell, `vendor/` 바이트 비교다. `src/db/**` Rust는 `fvoci-role-reviewer-db`다.
- 출력에 커밋 SHA, parent, `git diff --binary --full-index <parent> <SHA> | sha256sum`, 같은 diff의 `git patch-id --stable` 첫 필드, blocking / non-blocking, PASS/FAIL/NOTRUN/MISSING 목록이 있다.
- `cargo fmt --check`와 `cargo clippy --all-targets -- -D warnings`가 exit 0이다.
- `xtask`가 바뀌면 depth 1 checkout에서 `cargo test`가 저장소 루트와 `xtask/` 둘 다 PASS다.
- 의존성 변경에는 이유, 라이선스, lock diff가 있고 `cargo tree -d`에 새 중복이 없다. Rust가 주 리뷰어고 DB가 교차 리뷰어다.
- Python을 TypeScript 또는 Rust로 옮긴 변경은 이 리뷰어가 직접 만든 반례로 본다.
- 이미지·경로·ref·레지스트리 검사는 커밋 트리와 main 병합 결과 둘 다에서 한다.
- 리뷰 문장은 원격 CI를 대신하지 않는다. 결과 이름은 AGENTS.md 완료 조건이다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

그 SHA의 diff와 호출자만 본다. blocking이 있으면 REQUEST_CHANGES다. SHA가 바뀌면 바뀐 범위만 다시 본다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 작성자와 다른 주체다. AGENTS.md 허용 범위.
- 워크플로·웹은 `fvoci-role-reviewer-ci-web`이다.

## 출력

```
판정: ACCEPT | REQUEST_CHANGES
SHA: <40자>
parent: <40자>
patch-sha256: <git diff --binary --full-index parent SHA | sha256sum>
patch-id: <같은 diff | git patch-id --stable 의 첫 필드>
blocking:
- <한 줄 | 없음>
non-blocking:
- <한 줄 | 없음>
검사:
- <이름> PASS|FAIL|NOTRUN|MISSING
```

```
판정: REQUEST_CHANGES
SHA: cccccccccccccccccccccccccccccccccccccccc
parent: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
patch-sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
patch-id: dddddddddddddddddddddddddddddddddddddddd
blocking:
- src/api/documents.rs 빈 결과가 404다. 계약은 200과 빈 배열.
non-blocking:
- 없음
검사:
- cargo fmt --check PASS
- cargo clippy --all-targets -- -D warnings PASS
- vendor 바이트 커밋 트리 PASS
- vendor 바이트 main 병합 결과 NOTRUN
```
