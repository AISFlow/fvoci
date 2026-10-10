---
name: fvoci-role-reviewer-rust
description: "사람이 Rust 리뷰어 역할을 명시해 부를 때만 쓴다. 고정 SHA의 서버 계약을 보고 ACCEPT 또는 REQUEST_CHANGES를 돌려준다."
disable-model-invocation: true
---

# Rust 리뷰어

## 완료 조건

- 판정은 그 40자 SHA의 diff·호출자·계약에 대한 ACCEPT 또는 REQUEST_CHANGES다. ACCEPT에는 검토한 SHA가 적힌다.
- 근거는 `src/`·`crates/`의 실제 호출과 `fvoci-rust-slice` 완료 조건이다. 리뷰 문장은 원격 CI를 대신하지 않는다. 검사 결과는 AGENTS.md의 PASS, FAIL, NOTRUN, MISSING만 쓴다.

## 기본 절차

기본값이다. 완료 조건을 지키면 더 나은 경로로 벗어나도 된다.

그 SHA의 diff와 호출자만 본다. REQUEST_CHANGES에는 최소 수정안 한 가지를 적는다. SHA가 바뀌면 바뀐 범위만 다시 본다.

## 손대지 말 것

- 검토하는 커밋을 고치지 않는다. 작성자와 다른 주체여야 한다. AGENTS.md 허용 범위.
- 워크플로·웹·DB 스키마 판정은 각 리뷰어 스킬에 맡긴다.

## 출력

```
판정: ACCEPT | REQUEST_CHANGES
SHA: <40자>
범위: <본 경로>
근거: <호출 또는 계약 한 줄>
수정: <REQUEST_CHANGES일 때 한 줄, 아니면 없음>
```

```
판정: REQUEST_CHANGES
SHA: cccccccccccccccccccccccccccccccccccccccc
범위: src/api/documents.rs
근거: 목록 핸들러가 빈 결과를 404로 바꾼다. 계약은 200과 빈 배열이다.
수정: 빈 결과는 200과 빈 배열로 되돌린다.
```
