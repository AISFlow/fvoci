# web build handoff 의도와 호환성

원본은 제거한 `scripts/selected-backend-ci/web-build-handoff.py`와 그 검사
`scripts/fixtures/web-e2e/test-build-handoff.py`다. `scripts/run-web-e2e.sh`의 10개 호출이
`bun tools/selected-backend-ci/handoff.ts <mode>`를 쓴다. mode·exit 계약(성공 0, 거부 1,
stage는 child exit와 SIGINT 130)과 packet 파일 이름(`handoff.json`, `payload.tar`)은 같다.
원본이 거부하는 packet·입력을 새 구현이 허용하지 않는다.

| 항목            | 원래 동작                                           | 새 동작                                                                                     | 이유                                                                 |
| --------------- | --------------------------------------------------- | ------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| tar 읽기·쓰기   | Python `tarfile`                                    | node-tar 7.4.3 `Header`·`Parser`(strict, warn은 거부). 멤버 이름·type·mode·크기 검사는 같다 | `Bun.Archive`는 멤버 mode·type을 노출하지 않는다.                    |
| consumer 설치   | 검증 뒤 배타 생성                                   | 검증 패스 뒤 두 번째 패스에서 배타 생성하고 쓴 파일을 다시 hash한다                         | 두 패스 사이 packet 변경을 쓴 bytes로 잡는다.                        |
| 실패 진단       | Python 예외와 traceback                             | `web build handoff refused: <검사 메시지>`, 512자 제한, packet bytes 없음                   |                                                                      |
| 입력 차이 진단  | canonical JSON fingerprint                          | 같은 fingerprint와 512개 제한                                                               |                                                                      |
| 테스트 경계     | `patch.object`로 identity·tool·ldd·statfs 등을 교체 | `HostFacts`(checkout, 입력 수집기, toolchain 문자열, 여유 디스크)만 교체한다                | Git identity·clean tree·compiler env·ABI 파일·ldd는 실제로 실행한다. |
| ldd 반례        | 가짜 ldd 출력으로 not found·exit 1을 만든다         | 실제 ELF(`/usr/bin/true`, `/usr/bin/bash`)와 비 ELF 스크립트로 ldd 실패를 만든다            | 기록 쪽 반례는 receipt를 고쳐 만든다.                                |
| bytecode import | FreshImportTest                                     | 해당 없음                                                                                   | Bun은 import 때 checkout에 캐시를 쓰지 않는다.                       |
