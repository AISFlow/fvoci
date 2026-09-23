# FVOCI Orca 시작 패키지

작성 기준: 2026-09-24. 실제 사용자의 Orca·저장소·CLI에 설치하거나 실행한 패키지가 아니다. 프롬프트와 검토 후 적용할 텍스트 템플릿이다. 제품 코드나 실행 가능한 MCP 설정, 토큰, 외부 공식 스킬 복제본은 포함하지 않는다.

## 적용

1. `prompt.md`를 Orca의 codex Astra medium 코디네이터에 전달한다.
2. 코디네이터가 두 저장소와 기존 AGENTS/설정을 확인한 후 `project-template/` 내용을 검토·병합한다. 기존 파일에 압축을 곧바로 덮어쓰지 않는다.
3. `.agents/environment.md`의 실제 확인을 채우고, 공식 Orca 스킬의 현재 live guide를 읽는다. 모델·MCP CLI 옵션과 설정 형식을 추측하지 않는다.
4. 준비 검사 후 바로 첫 제품 기능을 구현한다. 환경 파일을 만든 것만으로 제품이나 설정 실행 검증 완료를 선언하지 않는다.

`AGENTS.md`는 운영 규칙, `.agents/environment.md`는 실제 연결/실행 기록, 5개 프로젝트 스킬은 필요할 때 읽는 절차다. 프로젝트 진행은 기존 `docs/rewrite.md`에 기록한다. 이 파일들의 전문을 서로 복제하지 않는다. 공식 Orca 스킬은 기존 설치를 재사용하고, 이 패키지에는 재구현하지 않았다.

## 구성

- prompt.md: 기존 전체 Rust 재작성 요구사항과 구체적인 0단계 운영 설정.
- project-template/AGENTS.md: 역할·파일 소유권·검증·권한·재개 규칙의 병합 템플릿.
- project-template/.agents/environment.md: 실행 경로·MCP 최소 구성·실제 확인용 템플릿.
- project-template/.agents/skills/: 5개 SKILL.md. 범용 스킬 설치기가 아니라 FVOCI 작업용 로컬 지침이다.

## 공식 참고 문서

아래 자료는 지원 기능을 확인한 근거다. 명령·기능은 사용자의 설치 버전에서 다시 확인한다. 모델 별칭 Astra/Fable/Composer 2.5/Grok 4.6의 실제 매핑은 사용자 환경에서 검증해야 한다.

- [Orca 공식 스킬과 MCP](https://www.onorca.dev/docs/cli/skills)
- [Orca orchestration](https://www.onorca.dev/docs/cli/orchestration)
- [Orca worktrees](https://www.onorca.dev/docs/model/worktrees)
- [Git worktree](https://git-scm.com/docs/git-worktree)
- [Codex 프로젝트 스킬](https://developers.openai.com/codex/skills)
- [Cursor 프로젝트 스킬](https://cursor.com/docs/skills)
- [Cursor CLI의 MCP](https://cursor.com/docs/cli/mcp)
- [GitHub 공식 MCP](https://github.com/github/github-mcp-server)
- [Context7](https://github.com/upstash/context7)
- [Playwright MCP와 CLI 선택](https://github.com/microsoft/playwright-mcp)
- [Cargo 환경 변수](https://doc.rust-lang.org/cargo/reference/environment-variables.html)

현재 공식 문서상 Orca의 공식 스킬은 실행 중인 CLI의 guide를 조회하는 방식을 사용하고, orchestration은 Run/Task/Dispatch 및 supervised worker 흐름을 제공한다. Cursor와 Codex는 프로젝트 .agents/skills 경로를 안내한다. 이를 사용자의 설치 환경에서 검증해야 하며, 읽기 전용 프롬프트나 worktree 자체가 OS 보안 격리를 강제하는 것은 아니다.

이 패키지는 MCP를 모두 설치하도록 요구하지 않는다. GitHub는 기존 도구/gh 우선, 문서는 공식 자료 우선, 브라우저는 기존 테스트/CLI 우선이다. 각 기능이 실제로 부족할 때만 제한된 MCP를 연결한다.
