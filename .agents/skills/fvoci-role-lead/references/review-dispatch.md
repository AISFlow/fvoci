# 리뷰 배정 절차

리드가 리뷰어를 띄우고 결과를 회수하는 절차다. 실행 환경과 무관하게 쓴다. 리뷰 수와 주 리뷰어는 AGENTS.md, 리뷰어의 범위와 출력은 각 `fvoci-role-reviewer-*`, 병합은 `fvoci-handoff`와 `fvoci-role-integrator`에 있다. 여기서 다시 적지 않는다.

## 환경 값

세션을 시작할 때 아래 값을 정하고 사용자에게 보고한다. 정할 수 없으면 추측하지 말고 묻는다. 값은 실행 환경 설정에 두고 레포에는 적지 않는다.

- 리뷰 worktree 상위 디렉터리, 리뷰 보고서 디렉터리, 테스트용 TMPDIR 상위 디렉터리(mode 0711)
- 무거운 명령의 lock 경로와 가용 메모리 하한
- 리뷰어 실행 방식과, 그 방식을 쓸 수 없을 때의 대체

디렉터리는 모두 tmpfs가 아닌 디스크에 둔다. tmpfs는 재시작하면 사라지고, inode 재사용 같은 CI 디스크 동작을 재현하지 못한다.

## 리뷰어 정하기

리드는 사용자에게 묻지 않고 diff에서 필요한 리뷰어를 정해 띄운다. 묻는 것은 리뷰어를 띄울 방법이 없을 때와, AGENTS.md가 메인테이너 말을 요구할 때뿐이다.

1. `git diff --name-only <base> <head>`로 바뀐 경로를 얻는다.
2. 경로마다 담당 역할을 정한다. 각 `fvoci-role-reviewer-*` 스킬의 "범위" 문장과 대조한다. 그 범위를 여기에 다시 적지 않는다.
3. 필요한 역할의 합집합을 띄운다. 리뷰 수, 주·교차, 기본 리뷰어는 AGENTS.md "허용 범위"를 따른다. 이 절은 그 수나 기본 리뷰어를 줄이지 않는다.
   - 합집합이 AGENTS.md가 요구하는 수보다 적으면, 위험이 다음으로 큰 경계의 역할을 교차 리뷰어로 더한다.
   - 어느 리뷰어 범위에도 맞지 않는 경로(AGENTS.md, `.agents/` 등)는 AGENTS.md가 정한 기본 리뷰어를 쓴다. 문서만 바뀌어도 같다.
4. 주 리뷰어가 정해져 있지 않으면, 가장 위험한 변경을 맡는 역할을 주로 둔다. 위험은 저장·인가, 게이트·workflow, 그 밖의 순으로 본다.
5. 한 역할에서 리뷰어는 한 명이다. 같은 diff에 같은 검사를 하는 리뷰어를 둘 이상 두지 않는다.
6. 배정 결과를 보고에 적는다: 경로별 역할, 주·교차, 리뷰어 수의 근거가 된 AGENTS.md 항목.
7. head가 바뀌면 바뀐 경로로 이 절을 다시 적용한다. 새 경계가 생기면 그 역할을 추가로 띄운다.

## 띄우기

1. 대상을 고정한다. `git fetch origin` 뒤 `git rev-parse <ref>^{commit}`으로 40자 SHA를 얻는다.
2. `git worktree add --detach <상위>/rv-<리뷰id> <SHA>`로 리뷰 worktree를 만든다. HEAD가 그 SHA와 같고 `git status --porcelain`이 비어 있는지 확인한다.
3. 프롬프트를 쓴다. 해당 `fvoci-role-reviewer-*` 스킬과 아래 항목을 담는다. 리뷰어 셸에 없는 값(경로, lock, 하한)은 변수로 남기지 말고 실제 값을 펼쳐 넣는다.
4. 리뷰어를 그 worktree에서 시작한다. 사용자가 볼 수 있는 방식이 있으면 그 방식을 쓴다. Orca IDE에서는 `orca.md`를 따른다. 숨은 실행밖에 없으면 그렇다고 보고한다. 읽기 전용 권한을 실제로 강제할 수 없으면 제한했다고 주장하지 않는다.

## 프롬프트 항목

1. 역할, 주·교차 구분, 리뷰 id, 라운드.
2. 고정된 head 40자 SHA, base SHA, 브랜치.
3. 읽을 스킬. 바뀐 경로에 해당하는 절만 읽게 한다.
4. 변경 요약. 작성자의 주장이니 믿지 말고 검증하라고 적는다.
5. 2라운드부터: 이전 보고서, 다시 확인할 차단 항목, 그 뒤 바뀐 커밋. 리뷰는 차이분과 그것이 깨뜨릴 수 있는 곳으로 좁힌다.
6. 확인할 것:
   - `git diff --name-status <base> <head>`가 범위 안에 있는지.
   - 호출자 계약과 fail-closed 경로. 대상 함수나 CLI를 이름으로 적는다.
   - diff 밖의 소비처: 같이 바뀌어야 하는 lockfile, workflow, tsconfig, lint·format 목록, 호출자.
   - 호스트 차이: GitHub 러너 uid는 1001이고, 로컬 uid는 `id -u`로 확인한다. TMPDIR가 tmpfs인지도 확인한다.
7. 실행할 명령. cwd와 함께 정확히 적는다. 실제 CLI나 테스트로 반례를 재현하는 e2e를 넣는다.
8. 일부러 실행하지 않는 검사와 그 이유. 결과에는 NOTRUN으로 남는다.
9. 리뷰어 작업 공간: 리뷰 전용 TMPDIR와 `CARGO_TARGET_DIR`. probe나 mutation은 리뷰 worktree가 아니라 버리는 별도 worktree에서 하고, 끝나면 리뷰 worktree가 깨끗한지 확인하게 한다.
10. 판정은 정확한 40자 SHA에 대해서만 내린다. 보고서 경로를 적고, 보고서를 쓴 뒤 완료를 알리고 멈추게 한다.

## 무거운 명령

cargo build·test·clippy, docker build·compose, Playwright, 반복 실행은 호스트 전체에서 lock 하나를 거쳐 직렬로 실행한다. 가용 메모리가 하한 아래면 기다린 뒤 시작하고, `nice`로 낮춘다. 예: `flock <lock 경로> nice -n 10 cargo test --locked --offline`.

메모리 하한을 확인하는 일은 실행 환경의 래퍼가 맡는다. 여러 리뷰어가 동시에 cargo를 돌리다 호스트 메모리가 바닥난 적이 있다. 무거운 리뷰어 수도 함께 제한한다.

## 회수

- foreground로 오래 기다리지 않는다. 백그라운드 대기나 실행 환경의 완료 알림을 쓴다. 알림이 없으면 보고서 파일이 생기는지를 본다.
- 보고서를 직접 연다. 판정 줄과 검토한 40자 SHA가 둘 다 적혀 있어야 집계한다. 요약 메시지만 보고 ACCEPT로 세지 않는다.
- 끝난 리뷰어의 세션과 탭을 닫는다. 다음 라운드에 다시 쓰지 않을 worktree는 `git worktree remove`로 지운다.
- REQUEST_CHANGES를 받으면:
  1. 차단 항목을 재현해 사실인지 확인한다.
  2. 작성자에게 수정을 맡긴다.
  3. 새 head에 대해 차이분 리뷰를 띄운다. 이때 이전 보고서와 차단 항목을 넣는다.
- 라운드 제한과 차단 기준은 AGENTS.md를 따른다.

## 놓치기 쉬운 반례

- **diff 밖의 소비처.** path 의존성으로 바뀐 exact pin을 물려받는 다른 workspace는 자기 `Cargo.lock`이 옛 버전에 남아 있으면 manifest와 어긋나고, 그 workspace의 `cargo fetch --locked`가 실패한다. 바뀐 lock만 보면 이 실패를 놓친다.
- **uid 차이.** "다른 uid"를 1001로 고정한 테스트는 로컬 uid가 1001이 아니면 통과하지만, GitHub 러너(uid 1001)에서는 매번 실패한다.
- **tmpfs TMPDIR.** tmpfs에서는 inode 번호가 재사용되지 않는다. 그래서 TMPDIR가 tmpfs면 inode 재사용에 기대는 결함이 로컬에서 숨는다.

## 출력

```
배정: deps-rust-r2, Rust 주, 2라운드
대상: draft/deps cccccccccccccccccccccccccccccccccccccccc (base aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)
이전 차단: crates/document-extract/Cargo.lock libc가 client의 exact pin과 다르다.
확인: diff 범위, pin을 해석하는 모든 Cargo.lock, crates/document-extract의 cargo fetch --locked·clippy·test.
NOTRUN: 원격 CI(병합 검사에서 확인), PostgreSQL 앱 역할(DB 변경 없음).
보고서: <보고서 디렉터리>/review-deps-cccccccc-rust.md
회수: 판정 ACCEPT, SHA cccccccccccccccccccccccccccccccccccccccc가 보고서에 있다.
```
