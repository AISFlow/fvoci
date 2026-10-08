# FVOCI 버전 로드맵 — 1.0.0까지

이 문서에는 버전 묶음과 순서만 적는다. 각 항목의 범위와 수락 기준은 [rewrite.md](rewrite.md)의 해당 절이 정본이고, 여기서 다시 쓰지 않는다.
단, 버전 배치는 이 문서가 정본이다. rewrite.md §5.3은 Storybook·Playground·CJK 정리를 0.6.x에, 공개 베타를 0.7.0에 두지만, 사용자 결정(2026-10-08)에 따라 화면 다듬기를 0.7.0으로, 공개 베타를 0.8.0으로 옮긴다. §5.3의 해당 문장은 다음 승인 문서 커밋에서 이 배치로 고친다. 0.7.0 이후 범위는 rewrite.md에 해당 절이 생기기 전까지 이 문서의 버전 절이 정본이다.
현재 head·CI·담당은 PR 본문 체크포인트에서 확인한다. 진행 상황은 버전마다 GitHub 이슈 하나로 추적한다. 레포를 다시 만들면 이슈가 사라지므로 이슈를 범위의 근거로 쓰지 않는다.

## 버전 규칙

- 1.0.0 전까지는 유사 semver를 따른다. `0.y.0`은 기능 묶음이고, `0.y.z`는 그 묶음의 결함 수정과 문서만 담는다. 새 기능을 patch에 넣지 않는다.
- 발행은 [RELEASING.md](RELEASING.md)의 `v0.y.z` 절차를 따른다. main 병합, 태그, 릴리스, 배포는 버전마다 사용자 승인을 받는다.
- 1.0.0 발행은 아직 승인되지 않았고([AGENTS.md](../AGENTS.md) "대상 원격 반영 승인 범위"), release 도구도 `0.y.z`만 받는다(RELEASING.md "Version"). 1.0.0 직전에 둘 다 바꾼다.
- 한 버전의 종료 조건은 같은 고정 head에서 실제로 돈 CI로만 확인한다. 이전 SHA의 성공은 합산하지 않는다.

## 한눈에

| 버전 | 진행 이슈 | 목표 | 종료 조건 |
| --- | --- | --- | --- |
| 0.6.0 | #363 | PostgreSQL·로컬 SQLite·원격 libSQL/Turso에서 UI부터 복원까지 끝까지 연결 | rewrite.md §5.3의 "0.6.0 필수"가 모두 수락됨. 같은 head에서 필수 다섯 gate(rust·web·install·documents·collab-engine)와 Turso dispatch가 PASS. main 병합 뒤 main gate가 PASS이고 `v0.6.0` release가 통과 |
| 0.7.0 | #364 | 실제 제품 화면 다듬기(Storybook·Playground, CJK, 접근성)와 블록 단위 충돌 해결 | 0.7.0 절(신설)의 항목이 수락됨. 같은 head의 필수 gate PASS. Storybook 정적 빌드와 상호작용·접근성 검사가 CI에서 돌고, 충돌 해결이 세 backend에서 실제 경쟁 저장으로 확인됨 |
| 0.8.0 | #365 | 무료 공개 베타 준비와 실제 공개 | 0.8.0 절(신설)의 운영 수락이 같은 RC에서 통과함. 공개는 승인된 환경에서 외부 사용자 흐름까지 확인한 뒤에만 완료로 본다 |
| 1.0.0 | #366 | 지원 범위를 고정한 안정판 | 1.0.0 절(신설)의 출시 차단 결함이 0이고, 같은 RC에서 `upgrade-smoke`가 실제로 PASS(SKIP·NOTRUN은 불충분)하며, 베타 데이터에서 올라오는 upgrade가 CI로 검증됨. 1.y.z 발행 승인과 release 도구 반영 |

## 0.6.0 — 세 DB 지원

순서대로 진행한다. 앞 단계가 막으면 뒤 단계를 수락하지 않는다.

1. **CI 하네스 정리**: fast job부터 복구한 뒤 timeout·CI 부하를 손보고, Python·shell 하네스를 Rust 테스트·Playwright TS·표준 Actions로 옮긴다. 범위는 하네스 계획에서 확정한 다음 [fvoci-fast-verify](../.agents/skills/fvoci-fast-verify/SKILL.md)나 rewrite.md에 반영한다(현재 정본 절 없음).
2. **Turso 실행 경로 이전**: Turso 전체 실행을 PR 브랜치 dispatch로 옮기고 검증 브랜치를 정리한다([testing-turso.md](testing-turso.md)).
3. **제품 흐름**(각 단위를 세 backend에서): 신규 설치 12단계(rewrite.md §2.1) → 인증·로그아웃·철회(§3.1) → 개인 입력·프로젝트 문서·멤버 변경(§3.2) → ON 회귀와 OFF CAS·충돌·초안(§3.3) → 응답 유실·중복 command(§3.4) → 현재 데이터·리비전·정밀도(§3.5) → 재시작·반출·다른 설치본 복원(§3.6) → 같은 작업량 ON/OFF 자원 비교(§3.7).
4. **문서 정리**: 테스트 DB 초기화 허용 범위, Python 제거, README 현행화, RELEASING.md가 가리키는 기능표 정리, main에 남은 옛 에이전트 문서 정리.
5. **발행**: main 병합(승인) → main gate PASS → release-prep(버전 0.6.0) → `v0.6.0` 태그(승인).

## 0.7.0 — 화면 다듬기와 충돌 해결

0.6.0 수락 뒤 시작한다. rewrite.md §5.3은 이 묶음을 0.6.x에 두지만 위 버전 배치에 따라 0.7.0으로 옮긴다. 범위의 정본은 아래 항목이며, rewrite.md에 0.7.0 절을 새로 쓰면 그 절로 옮긴다.

1. Storybook·Playground: 제품과 같은 token·font·번역·formatter를 쓰고, 정적 카탈로그와 실제 서버 Playground를 구분한다.
2. CJK·반응형·접근성: 키보드, focus, 텍스트 확대, reflow, IME 회귀를 본다.
3. 블록 단위 충돌 해결 UI: §3.3의 OFF 버전 조건 저장 위에 얹는다.
4. 테스트 유지보수 후속: Vue 내부 필드에 기대는 테스트와 deprecated DOM 호환 코드를 정리한다.
5. 합성 데모 seed와 실제 기능 검증 흐름: Storybook·Playground와 같은 합성 fixture를 쓴다.

## 0.8.0 — 무료 공개 베타

1. 공개 범위·지원 DB·배포 모드와 알려진 한계를 정하고, rewrite.md §5.2의 외부 witness 중 베타에서 실제로 제공할 범위를 고른다.
2. 로그·오류 수집 경계와 삭제·보존 대상을 정리한다.
3. 같은 RC에서 부하 → 포화 → 회복, upgrade와 backup 복구를 연습한다.
4. 승인을 받은 뒤 공개하고, 공개 환경에서 외부 사용자 흐름까지 확인한다.

## 1.0.0 — 안정판

1. 베타에서 나온 출시 차단 결함을 모두 닫고, rewrite.md §5.1의 미결 정책 중 1.0에 필요한 것을 정한다.
2. 0.8.x 데이터에서 1.0.0으로 올라오는 upgrade 경로를 CI로 검증한다. 같은 RC에서 Container install의 `upgrade-smoke`가 선택 실행이 아니라 실제로 돌아 PASS해야 한다(SKIP·NOTRUN은 종료 조건 불충족).
3. 1.y.z 발행 승인을 받고, AGENTS.md·RELEASING.md·release 도구에 반영한다.

## 버전과 별개로 처리할 일

- 의존성 갱신 PR은 0.6.0 main 병합 뒤 한 번에 처리한다.
- 레포를 squash한 커밋 하나로 다시 만들기 전에 이슈·PR 링크에만 남은 근거를 문서나 artifact로 옮긴다.
