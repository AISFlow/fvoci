# FVOCI

FVOCI는 워크스페이스·프로젝트 단위로 문서(위키)를 함께 편집하고 작업·일정을 관리하는 협업 도구입니다.
이 저장소는 기존 FVOCI를 작은 Rust 서버와 Vue 3 웹 앱으로 다시 구현합니다.

- 서버: Rust(Tokio·axum·SQLx). 인증·세션·인가(PostgreSQL RLS), 실시간 공동 편집(Yrs, 격리된 helper 프로세스),
  첨부·문서 텍스트 추출, DOCX/PDF/PPTX/Markdown 내보내기, 검색(Meilisearch), 메일, 백업·복원 도구를 포함합니다.
- 웹: Vue 3 + Nuxt UI + Tiptap(`apps/web`, `packages/editor`, `packages/i18n`).
- 서버 런타임에는 Node가 없습니다. Bun은 웹 빌드·개발 도구로만 씁니다.

현재 0.x 개발 단계이며, 1.0 전까지 호환성과 지원 범위가 바뀔 수 있습니다. 승인된 다음 범위(0.6: PostgreSQL·로컬
SQLite·원격 libSQL/Turso)와 남은 수락 항목은 [docs/rewrite.md](docs/rewrite.md)를 따릅니다.

## 문서

| 목적                                        | 문서                                   |
| ------------------------------------------- | -------------------------------------- |
| 개발 환경, 빠른 검사, 테스트, 기여 절차     | [CONTRIBUTING.md](CONTRIBUTING.md)     |
| 설치, 환경 변수, 운영 명령, 업그레이드·복원 | [RUNNING.md](RUNNING.md)               |
| 릴리스 절차                                 | [docs/RELEASING.md](docs/RELEASING.md) |
| 현재 범위와 수락 상태                       | [docs/rewrite.md](docs/rewrite.md)     |
| AI 에이전트 작업 규칙                       | [AGENTS.md](AGENTS.md)                 |

## 개발 시작

필요한 도구, 처음 설정, 고정 SQLite 준비, 변경 종류별 검사와 Docker가 필요한 DB·브라우저 테스트는
[CONTRIBUTING.md](CONTRIBUTING.md)에 있습니다. 컨테이너 설치는 [RUNNING.md의 Container install](RUNNING.md#container-install)을 따릅니다.

## 라이선스

[MIT](LICENSE)
