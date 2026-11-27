# 내장 템플릿

내장 템플릿 7개가 사는 단 하나의 자리다. 서버가 이 폴더의 `NN-<키>.<언어>.json`과 `files/`의 파일을 바이너리에 넣는다(`src/app/templates.rs`의 `TEMPLATE_FILES`, `TEMPLATE_ASSETS`). 템플릿마다 언어별 파일이 하나씩이고(`ko`, `en`), 한 언어의 파일은 그 언어로만 쓴다.

- **글은 이 폴더의 템플릿 파일에서 손으로 고친다.** 두 언어의 파일을 함께 고친다: 속성·뷰·카드의 식별자와 뷰의 설정, 본문의 뼈대는 같고 글만 다르다. 서버의 템플릿을 고친 판으로 다시 넣게 하려면 `src/app/templates.rs`의 `DEFAULT_TEMPLATE_VERSION`을 올린다. 이미 만들어진 보드(계정의 환영 보드 포함)는 바뀌지 않는다.
- 파일을 더하거나 빼면 `TEMPLATE_FILES`·`TEMPLATE_ASSETS`와 서버 테스트(`tests/api/templates.rs`의 `TEMPLATES`), 웹앱 테스트의 키 목록과 수를 함께 고친다.
- **있는 기능만 말한다.** 환영 보드(`01-welcome`)는 보드, 위키, 파일 보기를 소개한다. 기능이 바뀌면 그 글도 고친다.
- 사람은 넣지 않는다: 사람 속성은 두되 값은 비우고, 멘션은 쓰지 않는다. 바깥 주소는 링크·이메일 속성의 값에 `example.com`만 쓰고 본문에는 쓰지 않는다. 실제 회사와 사람의 이름을 쓰지 않는다.
- `files/`의 그림과 문서는 직접 만든 작은 파일이다(각 2 KB 미만). 언어가 드러나는 문서는 언어별로 둔다.

## 테스트가 보는 것

- 서버(`src/app/templates.rs`의 테스트): 모든 파일이 읽히고 언어마다 환영 보드가 하나인지, 두 언어의 파일이 글만 다른지, 본문이 보이는 파일과 `files` 목록이 맞는지, 본문에 둘 수 있는 노드(`fvoci_model::body_node_types`)를 모두 보이는지.
- 웹앱(`apps/web/tests/tools/builtin-templates.test.ts`): 본문이 편집기의 검증을 지나는지, 언어별 글이 빠짐없는지, 바깥 주소와 날짜 값의 모양, 템플릿 안의 참조, 그리고 뷰·속성·노드·마크의 종류를 코드의 목록에서 읽어 템플릿이 모두 보이는지. 보이지 않는 종류는 그 테스트 안에 까닭과 함께 적는다.

## 템플릿 파일의 모양

모양은 서버의 `TemplateFile`(`src/app/templates.rs`)이 정하고, 모르는 멤버가 있으면 서버가 템플릿을 넣지 못한다.

```jsonc
{
  "key": "issue-tracker",         // 제목이 바뀌어도 변하지 않는 이름. 언어가 달라도 같다
  "language": "ko",               // 쓰인 언어
  "baseDate": "2026-12-01",       // 날짜의 기준일. 보드를 만들 때 이 날이 만드는 사람의 오늘이 되게 날짜를 옮긴다
  "onboarding": false,            // 새 계정이 받는 환영 보드만 true
  "board": { "id", "title", "description", "icon", "showDescription", "cardProperties": [{ "id", "name", "type", "options": [{ "id", "label", "color" }] }] },
  "views": [{ "id", "type": "view", "title", "fields": { /* viewType, groupBy, cardOrder, filter, sort, … */ } }],
  "cards": [{ "id", "type": "card", "title", "fields": { "icon", "isTemplate", "properties": { "<속성 id>": "…" } }, "body": { "type": "doc", "content": [] } }],
  "files": [{ "asset": "board.png", "name": "보드.png" }]   // files/의 이름과, 보일 이름
}
```

- **본문**(`body`)은 카드마다 문서 하나다. 그림과 파일 블록은 `fileId`에 `files/`의 이름을 적고, 서버가 넣을 때 저장된 이름으로 바꾼다. 본문이 보이는 파일과 `files`는 꼭 맞아야 한다.
- **식별자**는 한 템플릿 안에서 참조(뷰의 `cardOrder`·`groupBy`·`defaultTemplateId`, 선택지 값, 카드 관계, 본문의 카드 참조)를 잇는 데만 쓴다. 서버가 넣을 때 새 식별자로 바꾼다.
- **보드에 넣지 않은 것**: `isTemplate`·`templateVersion`·보드 종류·만든 사람과 시각. 서버가 넣을 때 정한다.
- **카드의 `isTemplate`**은 보드 안의 카드 템플릿이다. 뷰의 `defaultTemplateId`가 가리킨다.
