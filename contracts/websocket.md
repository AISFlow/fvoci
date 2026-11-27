# WebSocket `/ws`의 메시지

적은 때: 2026-11-11 04:14 KST(본문의 공동 편집, 6절은 2026-11-13 04:41 KST, 그 커서는 2026-11-14 23:20 KST, 다시 들기는 2026-11-15 04:10 KST, 보드의 있음 7절은 2026-11-13 19:41 KST. 서버가 구독을 끝낼 때의 알림, 3절은 2026-11-14 01:35 KST. 죽은 연결의 `ping`·`pong`, 3절은 2026-11-15 19:02 KST. 팀의 조작, 3절과 6절은 2026-11-14 06:55 KST). 서버가 **지금** 주고받는 것을 모양을 바꾸지 않고 적었다(구독과 알림은 `crates/server/src/realtime.rs`, 프레임을 읽고 쓰는 쪽은 `crates/server/src/ws.rs`, 핸드셰이크는 `crates/server/src/lib.rs`의 `handle_websocket`). OpenAPI는 WebSocket을 적지 못하므로 계약 파일 `contracts/openapi.json` 밖에 둔다.

- 메시지마다의 멤버와 타입은 기계가 읽는 `contracts/websocket.json`에 있다. 서버의 테스트(`crates/server/tests/api/websocket.rs`의 `messages_match_the_websocket_contract`, 본문의 공동 편집(6절)의 `doc…`은 `rooms_contract.rs`의 `the_messages_of_the_rooms_match_the_websocket_contract`)가 서버가 실제로 보낸 메시지를 그 파일과 견준다: 맨 위 멤버가 정확히 그 이름들이고, 실린 객체(`container`, `block` 등)의 멤버는 계약 파일의 같은 이름의 스키마에 있는 것들이다. 테스트가 보는 것은 **메시지**뿐이다. 명령(2절)과 `session`·`sent` 표시는 이 문서와 코드를 사람이 맞춰 둔다. 실린 객체의 멤버는 이름만 견주고 타입은 계약 파일의 스키마를 따른다. Binary 프레임(6절)의 바이트는 `crates/server/src/collab/frame.rs`의 테스트와 `rooms.rs`의 테스트가 따로 쓴 부호화로 견준다. 커서의 바이트는 편집기의 라이브러리(y-protocols, Yjs)로 찍은 `contracts/caret-vectors.json`에 서버(`collab/caret.rs`)와 웹앱의 테스트가 함께 견준다.
- 메시지가 싣는 보드, 블록, 멤버, 카테고리는 계약의 `Container`, `Block`, `ContainerMember`, `Category`와 같은 모양이다(`containerMemberRemoved`의 `member`는 `{containerId, userId}`만 싣는다). `Block`은 본문(`body`)을 싣는다.
- 이 문서의 모양을 바꾸는 일은 계약을 바꾸는 일이다. 바꿀 때는 이 문서와 `websocket.json`을 같은 커밋에서 고친다.

## 1. 핸드셰이크와 인증

주소는 `{base}/ws`이고 `GET`의 WebSocket 업그레이드만 받는다. 그 밖의 메서드는 405(`Allow`에 `GET`이 있다. `HEAD`가 함께 있을 수 있다), 업그레이드가 아닌 `GET`은 400이다. 둘 다 API의 오류 본문(`application/problem+json`의 `Problem`)이다.

누구의 연결인지는 **핸드셰이크에서 한 번** 정해진다. 연결한 뒤에 로그인하는 메시지는 없다(옛 `AUTH`는 모르는 명령으로 무시된다).

| 핸드셰이크 | 결과 |
|---|---|
| `Origin`이 있고 서버의 출처(`server_url`의 출처나 설정 `allowed_origins`)가 아니다 | 403 `forbidden`, `reason: foreign_origin`. 연결되지 않는다 |
| `Authorization: Bearer <세션 토큰>`이 있다 | 세션이면 그 사용자의 연결. 세션이 아니면 401 |
| `Origin`이 있고 세션 쿠키(`fvoci_session`, HTTPS에서 `__Host-fvoci_session`)가 있다 | 세션이면 그 사용자의 연결. 끝난 세션의 쿠키면 **익명 연결**(401이 아니다. 로그아웃된 브라우저도 공유 보드를 따라간다) |
| `Origin`이 있고 자격이 없다 | 익명 연결 |
| `Origin`이 없고 Bearer도 없다(브라우저가 아닌 클라이언트) | 401 |

- 둘 다 오면 Bearer를 쓴다.
- 브라우저는 같은 출처의 페이지에서 `new WebSocket(…)`으로 열면 쿠키를 저절로 싣는다. 스크립트가 토큰을 알 필요가 없다.
- 익명 연결은 공유 보드의 블록 구독(`subscribeBlocks`)과 보드·위키의 읽기 토큰의 방문 알림(`setPresence`, 7절)을 할 수 있다.
- **공유 보드는 설정 `allow_public_links`가 켜져 있을 때만 있다. 기본값은 꺼짐이고, 꺼져 있으면 어느 읽기 토큰도 맞지 않아 `subscribeBlocks`는 늘 무시된다.**

## 2. 클라이언트가 보내는 명령

명령은 텍스트 프레임의 JSON 객체 하나다. 멤버는 camelCase이고 없는 멤버는 빈 값으로 읽는다. **수락된 구독 명령(`subscribeWorkspace`, `subscribeBlocks`)에는 각각 `workspaceSubscribed`·`blocksSubscribed`로 답한다.** 거절했거나 무시한 구독, 모르는 명령에는 오라클을 남기지 않도록 알리는 메시지가 없다(디버그 로그에만 남는다). 서버가 구독을 끝낼 때는 알린다(`workspaceUnsubscribed`·`blocksUnsubscribed`, 3절). 구독의 권한을 저장소에서 읽지 못하면 연결을 코드 1013으로 닫는다(3절): 다시 연결해 다시 구독한다. 그 밖에 답이 있는 것은 `joinDoc`(6절)과 `ping`(3절의 "죽은 연결")이다.

| `type` | 세션 | 멤버 | 하는 일 |
|---|---|---|---|
| `subscribeWorkspace` | 필요 | `workspaceId` | 그 워크스페이스의 변경을 받는다(3절). 수락되면 `workspaceSubscribed`를 보내며 같은 워크스페이스를 다시 구독해도 한 번이다 |
| `unsubscribeWorkspace` | 필요 | `workspaceId` | 그만 받는다 |
| `subscribeBlocks` | 없어도 된다 | `workspaceId`, `readToken`, `blockIds` | 공유 보드의 블록들을 읽기 토큰으로 따라간다(명령당 중복 제거 후 최대 200개). `blockIds`가 모두 한 보드의 것이고 `readToken`이 그 보드의 공유 토큰이며 `workspaceId`가 그 보드의 워크스페이스여야 한다. 수락되면 중복 제거된 `blockIds`와 함께 `blocksSubscribed`를 보내고, 하나라도 어긋나면 명령 전체를 무시한다 |
| `unsubscribeBlocks` | 없어도 된다 | 같다 | 그 블록들을 그만 따라간다. 같은 검사를 한다(저장소에서 읽지 못하면 검사 없이 그만 따라간다) |
| `joinDoc` | 필요 | `workspaceId`, `cardId`, 문서를 쥔 채 다시 들 때만 `epoch`, `stateVector` | 카드·위키 페이지·학습 단원의 본문을 함께 고치는 방에 들어간다(6절). `cardId`는 카드·페이지·단원의 ID다(장의 블록은 방이 없다). 답은 `docJoined`나 `docRefused` |
| `leaveDoc` | 필요 | `doc` | `docJoined`가 알려 준 번호의 방에서 나온다. 답이 없다 |
| `setPresence` | 읽기 토큰이면 불필요 | `workspaceId`, `containerId`, `cardId`, `readToken` | 보드·위키·학습 방문과 연 카드·페이지·단원을 알린다(7절). 답이 없다 |
| `ping` | 없어도 된다 | 없음 | 연결이 아직 이어져 있는지 묻는다. 서버는 곧바로 `pong`으로 답한다(3절의 "죽은 연결"). 멤버를 더 싣지 않는다: 64바이트를 넘는 메시지는 `ping`으로 보지 않는다 |

- 세션이 필요한 명령을 익명 연결이 보내면 무시한다(`joinDoc`은 `docRefused`로 답한다, 6절).
- JSON이 아니거나, 멤버의 타입이 틀리거나(`workspaceId`가 수, `blockIds`가 목록이 아님 등), `type`을 모르면 그 명령만 무시하고 연결은 그대로다.
- `subscribeWorkspace`는 그 워크스페이스의 구성원(손님도 포함)의 것만 받아들이고, 그 밖의 것은 무시한다. 워크스페이스 ID는 `GET /api/v1/users/me/workspaces`에 있다.
- 검사와 구독 등록 사이에 권한 회수가 있으면 다시 검사한다(모두 8번까지). 한 번 받아들인 뒤 다시 검사에서 권한이 없으면 구독하지 않고 끝났다고 알린다. 그 구독(그 사용자의 그 워크스페이스·조직, 그 워크스페이스, 그 보드의 링크)에 닿는 회수만 다시 검사하게 한다: 다른 사용자의 로그아웃이나 다른 조직의 회수는 겹치지 않는다. 닿는 회수가 8번 모두 겹치면 코드 1013으로 닫는다.
- `blockIds`가 빈 목록이면 보드를 정할 수 없어 읽기 토큰이 맞지 않는 것으로 친다.
- 비밀번호를 받지 않는 조직(공급자의 `ssoRequired`)의 워크스페이스는 그 공급자로 만든 세션의 연결만 구독한다. 다른 세션의 `subscribeWorkspace`는 무시한다(HTTP의 401 `sso_required`에 해당한다. `getMyWorkspaces`의 `ssoRequired`가 미리 알려 준다). 조직이 `ssoRequired`를 켜면, 그 조직의 워크스페이스를 그 공급자로 만들지 않은 세션으로 구독하던 것을 서버가 끝내고 `workspaceUnsubscribed`(`sso required`)로 알린다. 연결과 다른 조직의 구독은 그대로다. 조직이 다시 비밀번호를 받게 돼도 무시된 구독이 저절로 살아나지는 않는다: 다시 구독한다.
- 내 공간(`kind`가 `personal`인 워크스페이스)은 계정에 비밀번호가 있으면 비밀번호를 보인 세션의 연결만 구독한다: 비밀번호로 만든 세션, 그리고 공급자로 만든 뒤 비밀번호를 확인한(`confirmPassword`) 세션. 확인하지 않은 공급자 세션의 `subscribeWorkspace`는 무시한다(HTTP의 401 `password_sign_in_required`에 해당한다. `getMyWorkspaces`의 `passwordRequired`가 미리 알려 준다). 세션이 비밀번호를 확인하면 그 세션의 연결은 닫히지 않고 그 자리에서 확인한 세션의 연결이 된다: 무시됐던 `subscribeWorkspace`를 다시 보내면 구독되고, 다른 탭의 연결도 같다(새 쿠키로 다시 연결할 일이 없다). 확인한 것은 세션과 함께 끝나고, 세션이 끝나면 그 연결은 닫힌다. 계정이 첫 비밀번호를 정하면(`setPassword`) 정한 세션의 연결은 열린 채로 내 공간의 구독이 `workspaceUnsubscribed`(`password sign-in required`)로 끝나고 내 공간의 방에서 `docLeft`로 나온다(그 계정의 다른 세션은 끝나고 그 연결은 닫힌다). 놓친 구독이 있어도 다음 변경을 보내기 전에 같은 메시지로 끝난다.

## 3. 서버가 보내는 메시지

메시지는 텍스트 프레임의 JSON 객체 하나이고 `type`이 종류다. 그 밖에 서버는 설정 `websocket_ping_seconds`(1–59초, 기본 30초)마다 Ping 프레임을 보낸다. 첫 Ping은 연결하자마자 간다. 클라이언트(브라우저)가 Pong으로 답하고, 서버는 그것을 클라이언트가 보낸 것으로 센다(아래 "죽은 연결").

| `type` | 싣는 것 | 누가 받나 |
|---|---|---|
| `containerChanged` | `workspaceId`, `container`(`Container` 전체) | 그 워크스페이스를 구독한 연결 가운데 사용자가 보드를 읽는 사람인 것(아래 "받는 사람의 규칙") |
| `blockChanged` | `workspaceId`, `block`(`Block`). **카드·페이지·일기 항목·단원(`BlockType::WITH_ROOM`)은 본문 대신 그 요약을 싣는다**: `block`의 `body`는 `null`이고, 곁의 `summary`가 `hasBody`·`tasksTotal`·`tasksDone`(`getCards`의 카드의 그것)과 `preview`(`getCards`의 `preview=true`가 주는 `CardPreview`)를 싣는다. 본문은 방(6절)이나 HTTP로 읽는다. 다른 블록(댓글은 본문까지 블록 전체)과 지운 블록(아래 `deleteBlock`)에는 `summary`가 없다 | 위와 같은 연결, 그리고 그 블록이나 그 블록의 부모를 `subscribeBlocks`로 따라가는 연결 |
| `containerMemberChanged` | `workspaceId`, `member`(`ContainerMember`) | 그 워크스페이스를 구독한 연결 가운데 사용자가 보드를 읽는 사람인 것 |
| `containerMemberRemoved` | `workspaceId`, `member`(`{containerId, userId}`) | 위와 같은 연결, 그리고 빠진 사용자의 연결 |
| `userChanged` | `workspaceId`, `userId`. 이름은 싣지 않는다: 받은 클라이언트가 그 사람을 다시 읽는다(`getWorkspaceUsersById`) | 그 워크스페이스를 구독한 연결 가운데 사용자가 그 사람을 이름으로 보는 것(`permissions::sees_person`: 구성원은 모두, 손님은 같은 보드에 있는 사람). 그 사람 자신의 연결도 |
| `categoryChanged` | `workspaceId`, `category`(`Category`) | 카테고리 주인의, 그 워크스페이스를 구독한 연결 모두(아래) |
| `containerCategoriesChanged` | `workspaceId`, `containerCategories`(`{containerId, categoryId, hidden}`의 목록). **목록은 바꾼 항목들이지 전체가 아니다**: `setContainerCategory`와 보드를 기본 카테고리에 넣는 조작은 그 카테고리의 보드 전부를, `hideContainer`·`unhideContainer`·`restoreContainer`는 항목 하나를 싣는다. 받는 쪽은 항목마다 덮어쓴다(목록으로 갈아 끼우면 보드를 잃는다) | 그 사용자의 연결 모두 |
| `categoriesReordered` | `workspaceId`, `categoryOrder`(카테고리 ID의 새 순서) | 그 사용자의 연결 모두 |
| `categoryContainersReordered` | `workspaceId`, `categoryId`, `containerOrder`(그 카테고리의 보드 ID의 새 순서) | 그 사용자의 연결 모두 |
| `clientConfigChanged` | `clientConfig` | 모든 연결. **지금은 보내는 곳이 없다**(서버 설정이 실행 중에 바뀌지 않는다) |
| `workspaceSubscribed` | `workspaceId` | `subscribeWorkspace`가 수락된 그 연결 |
| `blocksSubscribed` | `workspaceId`, `blockIds`(중복 제거·정렬된 수락 블록 ID 목록) | `subscribeBlocks`가 수락된 그 연결 |
| `workspaceRoleChanged` | `workspaceId`, `userId`, `role`(`WorkspaceRole`) | 워크스페이스 역할이 바뀐 당사자의 연결 |
| `workspaceUnsubscribed` | `workspaceId`, `reason`(`workspace access ended`, `sso required`, `password sign-in required`) | 서버가 그 워크스페이스의 구독을 끝낸 연결(아래 "서버가 구독을 끝낼 때"). 연결과 다른 구독은 그대로다(방은 아래 "서버가 구독을 끝낼 때") |
| `pong` | 없음 | `ping`을 보낸 연결. 곧바로 간다: 그 연결이 앞서 보낸 것의 답(`docJoined` 같은 것)보다 먼저 올 수 있다. 연결의 보낼 줄(512개)이 차 있으면 가지 않는다 |
| `notificationsChanged` | 없음. 무엇이 바뀌었는지 싣지 않는다: 받은 클라이언트가 알림을 다시 읽는다(`getUnreadNotifications`, `getNotifications`) | 그 사용자의 세션이 있는 연결 모두(구독과 상관없이). 알림 작업이 알림을 만들거나 늘리면 그 알림의 조직을 그 연결의 세션이 여는 것만(`getNotifications`와 같은 거름: 프로바이더 세션만 받는 조직의 알림은 다른 세션의 연결에 아무것도 내지 않는다). 사용자가 읽음으로 바꿀 때도 같다: 읽은 알림의 조직을 여는 세션의 연결만 |
| `blocksUnsubscribed` | `workspaceId`, `blockIds`(그 보드의 따라가던 블록 모두. 받지 않은 구독이면 그 명령의 블록), `reason`(`public link access ended`, `public link connections full`) | 서버가 그 보드의 블록 구독을 끝낸 연결, 또는 받지 않은 구독을 보낸 연결 |

방문의 `presenceChanged`는 구독과 별개이며 받는 사람과 멤버는 7절을 따른다.

### 받는 사람의 규칙

- "보드의 멤버"는 메시지를 보낼 때 유효한 멤버 목록이다: 직접 역할을 받은 사람과 보드에 역할이 있는 팀의 사람. 그 조직의 구성원·역할·팀·공유 상태가 바뀌면 무효화되는 캐시(조직마다 따로이고, 휴지통 비우기 같은 시스템의 집안일은 건드리지 않는다)로 검사하며 워크스페이스 역할도 다시 확인한다. 워크스페이스를 구독하지 않은 연결은 멤버여도 받지 않는다.
- "보드를 읽는 사람"은 보드의 멤버이고, **워크스페이스에 열린 보드·열린 위키·열린 학습**이면 그 워크스페이스의 구성원(손님 아님) 모두다: 열린 보드·열린 위키·열린 학습은 참여하지 않고 읽는다(`App::container_access`와 같은 규칙). 열린 템플릿은 멤버만이다. 권한을 바꾸는 DB 쓰기가 취소되어 완료 여부를 모르면 그 뒤 60초 동안은 캐시를 쓰지 않고 보낼 때마다 저장소에서 다시 읽는다. 확인된 롤백은 이에 해당하지 않는다.
- **카테고리의 네 메시지는 그 사용자의 연결 가운데 그 워크스페이스를 구독한 것 모두에 간다.** 같은 사용자의 다른 탭도 받는다.
- `subscribeBlocks`로 따라가는 연결 가운데 읽기 토큰만으로 읽는 독자(세션 없는 연결, 보드의 역할이 없거나 그 워크스페이스에 없거나 세션이 그 조직을 열지 못하는 사용자)가 받는 `blockChanged`은 HTTP의 `getBlocks`가 그 독자에게 주는 모양이다: `createdBy`, `modifiedBy`, 댓글의 본문의 모든 멘션의 `userId`, 사람 속성이 가리키는 사람이 비어 있다. 요약의 미리 보기 글에는 멘션과 참조가 아무것도 내지 않는다. 사용자 ID가 가입 시각을 드러낸다. 그 보드를 멤버로 보는 사용자(보드의 역할이 있고, 그 워크스페이스에 있고, 세션이 그 조직을 연다)의 연결은 같은 링크로 따라가도 멤버가 받는 모양을 받는다.
- 한 연결이 워크스페이스 구독과 블록 구독을 함께 해서 두 길로 다 해당하면 메시지를 두 번 받는다(보드를 멤버로 보면 둘 다 멤버의 모양, 아니면 블록 구독의 것은 사용자 ID를 비운 모양).
- 쓴 사람 자신의 연결도 받는다. 서버는 보낸 사람을 빼지 않는다.

### 메시지를 내는 조작

| 조작(계약의 `operationId`) | 메시지 |
|---|---|
| `createBoard`, `createContainersAndBlocks`, `duplicateContainer`(템플릿에서 만들기 포함), `onboard` | 새 보드마다 `containerChanged`, 블록마다 `blockChanged`, 멤버마다 `containerMemberChanged`. 템플릿이 아닌 새 보드는 만든 사람의 기본 카테고리에 들어가며 `containerCategoriesChanged`를 낸다. `onboard`는 보드를 고친 뒤 `containerChanged`를 한 번 더 낸다 |
| `importArchive` | 보드·블록의 `containerChanged`·`blockChanged`이 멤버가 생기기 전에 나가 **아무도 받지 않는다**. 그 뒤 `containerMemberChanged`, `containerCategoriesChanged`, 이미지를 고친 블록의 `blockChanged` |
| `patchContainer` | `containerChanged`. **템플릿**의 패치에 `type`이 있으면(바뀌지 않았어도) 멤버가 아닌 워크스페이스 사용자마다: 비공개면 `containerMemberRemoved`, 공개면 보기 역할의 `containerMemberChanged`. 템플릿이 아닌 보드는 종류가 바뀌어도 `containerChanged`뿐이다. 다만 **열린 보드·열린 위키·열린 학습**을 비공개로 바꾸면 멤버 아니게 읽던 사람의 연결에 `deleteContainer`와 같은 `containerChanged`(`deletedAt`이 있음)가 간다: 그 사람에게는 보드·위키·학습이 없어졌다 |
| `patchContainersAndBlocks` | 바뀐 블록마다 `blockChanged`, 바뀐 보드마다 `containerChanged` |
| `reorderCardProperties` | 새 순서의 `cardProperties`를 실은 `containerChanged` |
| `deleteContainer`, `deleteContainersAndBlocks` | `containerChanged`, `container`는 `id`, `workspaceId`, `updatedAt`, `deletedAt`만 뜻이 있고 나머지는 빈 값. 받는 사람은 지우기 직전에 그 보드를 읽던 사람(멤버, 열린 보드·열린 위키·열린 학습이면 워크스페이스의 구성원도). `deleteContainersAndBlocks`는 그 앞에 블록마다 삭제 `blockChanged`(모양은 아래 `deleteBlock`)을 내는데, 이것도 그 사람들에게만 가고 `subscribeBlocks`로 따라가던 연결은 받지 않는다. 둘 다 보드의 공유 토큰으로 따라가던 블록 구독을 끝낸다(`blocksUnsubscribed`) |
| `restoreContainer` | 돌아온 보드의 `containerChanged` 하나, 그리고 보드를 사이드바에 다시 둔 사용자마다 그 사람의 연결에 `containerCategoriesChanged`(그 보드의 항목 하나: 원래 카테고리, 그사이 지워졌으면 기본 카테고리)와 그 카테고리의 `categoryContainersReordered`(원래 자리로 들어가며 뒤의 보드가 밀린 순서). 비공개 보드는 다시 구성원이 된 사람에게만. **함께 돌아온 블록과 멤버의 메시지는 없다** |
| `createBlocks`, `patchBlock`, `patchBlocks`, `createCard`, `patchCard`, `duplicateBlock`, `restoreBlock` | 바뀌거나 생긴 블록마다 `blockChanged`. `restoreBlock`은 되살린 블록 하나만 알린다(함께 돌아온 아래 블록은 알리지 않는다) |
| `deleteBlock` | `blockChanged`, `block`은 `id`, `containerId`, `type`, `updatedAt`, `deletedAt`만 뜻이 있고 나머지는 빈 값. **지운 블록 하나만** 알린다. 함께 휴지통에 간 아래 블록(카드의 댓글·첨부)은 따로 알리지 않는다 |
| `createWiki`, `createPage`, `patchPage`, `deletePage`, `createPageComment`, `patchPageComment`, `deletePageComment`, `restorePageComment`, `restorePageBodyVersion` | 위키는 보드의 한 종류(`kind`가 `wiki`)이고 페이지는 블록(`type`이 `page`)이어서 같은 메시지가 나간다: `createWiki`는 `createBoard`와 같고(`containerChanged`의 `container.kind`가 `wiki`), `createPage`·`patchPage`는 그 페이지의 `blockChanged`(아이콘과 순서는 `fields`의 `icon`·`order`), `deletePage`는 `deleteBlock`과 같다(지운 페이지 하나만. 함께 휴지통에 간 아래 페이지와 그 댓글은 따로 알리지 않는다). 위 표의 보드 조작 가운데 위키도 받는 것(`patchContainer`, `deleteContainer`, `restoreContainer`, 멤버·팀)은 위키에도 같은 메시지를 낸다. 페이지의 댓글은 카드의 댓글과 같다: `createPageComment`·`patchPageComment`는 그 댓글(`type`이 `comment`, `parentId`가 페이지)의 `blockChanged`, `deletePageComment`는 `deleteBlock`과, `restorePageComment`는 `restoreBlock`과 같다. `restorePageBodyVersion`은 `restoreCardBodyVersion`과 같다(그 페이지의 `blockChanged`, 방에는 새 문서) |
| `createCourse`, `createUnit`, `patchUnit`, `moveUnit`, `deleteUnit`, `restoreUnit`, `createUnitComment`, `patchUnitComment`, `deleteUnitComment`, `restoreUnitComment`, `restoreUnitBodyVersion` | 학습은 그릇의 한 종류(`kind`가 `course`)이고 장·단원은 블록(`type`이 `chapter`·`unit`)이다: `createCourse`는 `createBoard`와 같고(`containerChanged`의 `container.kind`가 `course`), `createUnit`·`patchUnit`·`moveUnit`은 그 블록과 순서가 다시 매겨진 형제의 `blockChanged`, `deleteUnit`은 `deleteBlock`과 같다(지운 장이나 단원 하나만; 함께 휴지통에 간 장 아래 단원과 단원의 댓글은 따로 알리지 않는다), `restoreUnit`은 돌아온 장·단원·댓글과 순서가 바뀐 형제의 `blockChanged`을 낸다. `patchContainer`, `deleteContainer`, `restoreContainer`, 멤버·팀 조작도 학습에 같은 메시지를 낸다. 단원의 댓글은 카드·페이지의 댓글과 같다: `createUnitComment`·`patchUnitComment`는 그 댓글(`type`이 `comment`, `parentId`가 단원)의 `blockChanged`, `deleteUnitComment`는 `deleteBlock`과, `restoreUnitComment`는 `restoreBlock`과 같다. `restoreUnitBodyVersion`은 `restoreCardBodyVersion`과 같다(그 단원의 `blockChanged`, 방에는 새 문서) |
| `addMember`, `joinContainer`, `updateMember` | `containerMemberChanged`. 이미 멤버인 사용자의 `addMember`·`joinContainer`, 멤버가 아닌 사용자의 `updateMember`는 아무것도 내지 않는다. 템플릿이 아닌 보드에 새로 든 멤버는 기본 카테고리에 들어가며 `containerCategoriesChanged`도 받는다 |
| `removeMember`, `leaveContainer` | `containerMemberRemoved` |
| `addTeamMember`, `removeTeamMember`, `deleteTeam`, `setContainerTeam`, `removeContainerTeam` | 역할이 바뀐 보드와 사람마다 `containerMemberRemoved`(역할이 생기거나 오른 때에도): 받은 클라이언트는 보드의 멤버를 다시 읽는다. 사람을 넣고 빼면 그 팀이 걸린 보드마다 그 사람의 것, 팀을 지우면 그 팀이 걸린 보드마다 그 팀의 사람 모두의 것, 보드의 팀 역할을 주고 바꾸고 떼면 그 보드에서 그 팀의 사람 모두의 것이다. 팀이 이미 그 역할이었거나 뗄 역할이 없었으면, 이미 팀에 있는 사람을 넣거나 팀에 없는 사람을 빼면, 또 팀이 걸린 보드나 팀의 사람이 없으면 아무것도 내지 않는다 |
| `createCategory`, `updateCategory`, `deleteCategory` | `categoryChanged`. 지운 카테고리는 `deletedAt`이 있다. `deleteCategory`는 그 보드들을 기본 카테고리로 옮기며 `containerCategoriesChanged`도 낸다 |
| `getCategories`와 기본 카테고리를 쓰는 위의 조작 | 사용자에게 기본 카테고리가 아직 없으면 만들면서 `categoryChanged`와 `containerCategoriesChanged`를 낸다(읽기인 `GET`도) |
| `reorderCategories` | `categoriesReordered` |
| `setContainerCategory`, `hideContainer`, `unhideContainer` | `containerCategoriesChanged` |
| `reorderCategoryContainers` | `categoryContainersReordered` |
| `setPublicLink`(끄기, 새 토큰) | 옛 토큰으로 따라가던 연결에 `blocksUnsubscribed` |
| `patchMe` | 이름이 바뀌면 그 사람이 구성원인(손님 포함) 워크스페이스마다 `userChanged`. 바꾸는 것이 없는 요청은 아무것도 내지 않는다 |
| `leaveOrganization` | 나간 사람의 연결 가운데 그 조직의 워크스페이스를 구독한 것에 워크스페이스마다 `workspaceUnsubscribed`(`workspace access ended`). 그 워크스페이스들의 방이 나간 사람을 다시 묻는다(`docLeft`). 나간 사람이 혼자 관리하던 보드가 워크스페이스의 다른 관리자에게 넘어가면 그 보드의 `containerMemberChanged`(새 관리자). 워크스페이스에서 사람을 뺄 때처럼 다른 사람에게 그 사람이 빠졌다는 메시지는 없다 |
| `patchUserSettings`, `patchClientState` | 메시지 없음 |
| `readNotification`, `readNotifications`, 알림 작업이 알림을 만들거나 늘릴 때 | 그 사용자의 연결에 `notificationsChanged`(위의 규칙) |

메시지는 쓰기가 끝난 뒤 보낸다. 어떤 조작은 답하기 전에 보내고(`createContainersAndBlocks`, `createBlocks`, `createCard`, `deleteContainer`, `deleteContainersAndBlocks`, 카테고리의 메시지 모두), 나머지는 따로 돌며 보낸다. 클라이언트는 HTTP의 답과 메시지의 순서에 기대지 않는다.

### 서버가 구독을 끝낼 때

권한 회수는 연결을 닫지 않고 그 구독만 끝내며 알린다. 연결은 남고, 다른 워크스페이스의 구독과 블록 구독도 남는다. 구성원·역할·관리자 참여가 바뀌거나 워크스페이스가 휴지통에 가면 그 조작이 답하기 전에 그 워크스페이스의 방이 다시 물어, 권한이 없는 사람은 `docLeft`로 내보내고 쓰기·읽기가 바뀐 사람에게는 `docModeChanged`를 보낸다. 다른 워크스페이스의 방은 남는다. `ssoRequired`가 켜지면 그 공급자로 만들지 않은 세션은 그 조직의 모든 방에서 저장소에 묻지 않고 나간다(6절의 "방에 있는 동안"). 클라이언트는 다시 구독해 보고(권한이 있으면 새 역할로 받아들여진다) 보드를 다시 읽는다.

| 언제 | 끝나는 구독, 알림 |
|---|---|
| 워크스페이스에서 빠짐·역할 변경, 관리자 참여 회수·강등이나 인스턴스 관리자 해제로 가입 당시 역할을 잃음 | 그 사용자의 그 워크스페이스 구독(`workspaceUnsubscribed`, `workspace access ended`) |
| 워크스페이스가 휴지통에 감 | 그 워크스페이스의 모든 구독(같은 알림) |
| 조직이 비밀번호를 받지 않게 됨(`ssoRequired`) | 그 조직의 워크스페이스를 그 공급자로 만들지 않은 세션으로 구독하던 것(`sso required`). 조직의 워크스페이스를 읽지 못하면 그런 세션의 워크스페이스 구독 모두 |
| 공유를 끄거나 새 토큰을 내거나 보드·워크스페이스를 휴지통에 넣거나 조직의 공개 링크를 끔 | 그 보드의 옛 읽기 토큰으로 따라가던 블록(`blocksUnsubscribed`, `public link access ended`) |
| 보낼 때의 검사가 권한이 끝났거나 역할이 바뀌었거나 세션이 그 조직을 열지 못함을 봄(앞의 알림이 없던 회수) | 그 연결의 그 구독(같은 알림) |
| 공개 링크로 그 보드를 읽는 연결이 이미 `public_link_connections_per_container`(기본 200)개, 또는 그 가운데 같은 주소(IPv6는 /64)의 것이 `public_link_connections_per_address`(기본 10)개 | 새로 오는 그 연결의 블록 구독을 받지 않는다(`blocksUnsubscribed`, `public link connections full`. `docs/configuration.md`). 읽기 토큰으로 블록을 따라가거나 방문(7절)하는 연결을 하나로 센다. 이미 읽는 연결은 막지 않고, 구성원이 보통으로 여는 연결(읽기 토큰 없이)은 세지도 막지도 않는다. 받지 않은 구독은 다시 보내지 않으면 그대로이고, 방문은 30초마다의 갱신에 자리가 있으면 받는다 |

### 죽은 연결

조용히 죽은 연결(망이 바뀜, 노트북을 덮음)은 양쪽 다 한참 동안 알지 못한다. 브라우저의 스크립트는 WebSocket의 Ping과 Pong을 볼 수 없으므로 클라이언트가 `{"type":"ping"}`을 보내 묻고, 서버는 `{"type":"pong"}`으로 답한다.

- **서버.** `ping`은 그 연결이 맡긴 다른 일(즉시 재확인을 기다리는 방의 변경 같은 것)을 기다리지 않고 곧바로 답한다: 서버는 일하는 동안에도 연결을 읽어, `ping`에 답하고 나머지는 온 차례대로 쥐어 두었다가 처리한다(1 MiB(그것을 넘게 한 메시지까지)나 4,096개까지. 넘으면 일이 끝날 때까지 더 읽지 않고, 그동안의 `ping`에는 답하지 못한다). `ping`은 방의 프레임 한도(`collab_frames_per_second`)에 세지 않고 세션이 없어도 된다. 서버는 **90초** 동안 아무것도(텍스트 메시지, Binary 프레임, `ping`, WebSocket의 Ping이나 Pong) 받지 못한 연결을 닫는다. 연결이 맡긴 일을 하는 동안은 세지 않고, 그 일이 끝나면 다시 센다. 닫힌 연결은 방에서 나가므로(다른 연결은 그 커서가 거둬졌다고 받는다), 서버가 죽은 줄 모르던 연결이 방의 자리와 한도(`collab_connections_per_document`, `collab_connections_per_user`)를 쥐는 것은 90초까지다. 그 커서는 받는 쪽이 30초 뒤에 이미 지운다(6절의 "커서"). 한 사용자의 탭 여럿이 함께 끊겨 다시 들면, 옛 자리가 빠질 때까지 `collab_connections_per_user`에 걸려 `docRefused`(`too_many_connections_joined`)를 받는 탭이 있을 수 있다(물러서며 다시 든다).
- **웹앱.** 연결이 열리고 20초 뒤, 그 뒤로는 답을 받고 20초 뒤에 `ping`을 보내고, 15초 안에 서버에게서 아무것도 오지 않으면 그 연결을 놓고 다시 연결한다. 답은 메시지가 다 와야 오므로(느린 망의 큰 문서 하나는 15초보다 오래 걸린다) 답 없이 놓을 때마다 다음 연결의 기다림을 두 배로(1분까지: 서버가 90초 동안 아무것도 받지 못하면 닫는다) 늘리고, 답을 받으면 15초로 돌린다. 화면은 끊김과 같다: 본문은 읽기 전용이고, 다시 들면 상태 벡터로 차이를 주고받는다(6절). 기다림이 정한 때보다 1초 넘게 늦게 끝났으면(뒤의 탭, 잠들었던 페이지, 오래 걸린 스크립트) 페이지가 잔 것이지 서버가 없는 것이 아니므로 한 번 더 묻고, 그것에도 답이 없어야 놓는다.
- **뒤의 탭.** 브라우저는 뒤의 탭의 타이머를 늦춘다(크롬은 5분 넘게 가려진 탭을 분에 한 번까지). 그런 탭의 `ping`은 1분쯤마다 가고, 서버의 Ping에는 브라우저가 타이머와 상관없이 Pong으로 답한다. 90초는 둘 모두보다 길다. 설정 `websocket_ping_seconds`는 느린 망의 Pong까지 그 안에 들도록 59초까지만 받는다(0이나 60 이상이면 서버가 시작하지 않는다).

### 서버가 연결을 닫을 때

세션이 끝나면 코드 1008과 이유 `session ended`를 담은 종료 프레임을 보낸다. 받을 사람의 권한을 저장소에서 읽지 못했을 때는 권한이 끝난 것이 아니므로 코드 1013과 이유 `try again`으로 닫는다: 클라이언트는 놓친 변경 없이 다시 연결해 다시 읽는다. 대기 중인 변경은 버린다. 전송 중인 프레임을 취소한 때는 그 프레임이 나가지 않도록 연결을 바로 끊으며, 핸드셰이크 직후 세션이 끝났거나 종료를 5초 안에 마치지 못한 때도 프레임 없이 끊는다. 클라이언트는 다시 연결하고 보드를 다시 읽는다.

| 언제 | 닫히는 연결 |
|---|---|
| 로그아웃 | 그 세션으로 연 연결 |
| 관리자가 비밀번호를 재설정 | 그 사용자의 모든 연결 |
| 자기 비밀번호를 바꾸거나 첫 비밀번호를 정함 | 그 사용자의, 바꾼 세션이 아닌 세션의 연결. 바꾼 세션의 연결은 쿠키가 바뀌어도 닫히지 않는다 |
| 핸드셰이크 직후, 연결을 등록하기 전에 세션이 끝나 있음 | 그 연결(닫는 프레임 없이) |
| 세션이 한가함 한도나 절대 한도로 끝나거나 사용자가 지워짐 | 그 세션의 연결. 서버가 Ping마다(`websocket_ping_seconds`) 세션을 다시 보고 닫는다 |
| 보낼 변경이나 구독의 권한을 저장소에서 읽지 못함(보드의 멤버를 읽지 못하면 그 워크스페이스를 구독한 연결 모두), 또는 구독이나 변경을 8번 검사해도 그 사이의 권한 쓰기가 겹침(변경은 마지막 검사에서 받을 연결만) | 그 연결(코드 1013). 저장소가 내려가 있으면 많은 연결이 함께 다시 연결한다: 클라이언트는 물러서며 다시 연결한다 |
| 보내지 못한 메시지가 512개 쌓임 | 그 연결(읽지 않는 클라이언트) |
| 받은 텍스트 메시지가 64 KiB를 넘거나, Binary 프레임이 64 KiB(설정 `collab_update_bytes`에 32바이트를 더한 것이 더 크면 그것)를 넘음 | 그 연결 |
| 방(6절)이 보낸 것을 전혀 읽지 않아, 방에서 내보냈다는 알림만으로 기다리는 메시지가 `collab_queue_messages`의 두 배(기본 1,024개)를 넘음 | 그 연결 |
| 90초 동안 아무것도 보내지 않음(위 "죽은 연결") | 그 연결 |
| 보낸 변경(6절)을 받다가 문서의 작업자가 끝남(`collab_worker_timeout_ms`를 넘었거나 작업자가 깨졌다) | 그 변경을 보낸 연결(닫는 프레임 없이). 그 사용자의 변경은 10초부터 두 배씩 1시간까지 `document_unavailable`로 거절되고(그 사이가 하루를 넘으면 처음부터 다시 센다), 같은 조직에서 하루 안에 그런 사용자가 세 번째인 때부터는 그 조직 모두의 것도 거절된다 |
| 서버가 멈춤 | 모든 연결 |

## 4. 알아 둘 것

- **놓친 변경을 다시 보내지 않는다.** 순번도 판 번호도 없다. 다시 연결한 클라이언트는 필요한 것을 HTTP로 다시 읽는다.
- **`updatedAt`은 서버의 밀리초 시각이다.** 있는 블록을 고칠 때마다 블록의 `updatedAt`은 앞의 값보다 커진다(같은 밀리초의 쓰기나 시계가 뒤로 간 때에는 앞의 값에 1을 더한다). 그래서 `patchCard`의 `readAt`이 그 사이의 변경을 놓치지 않는다. 휴지통에서 돌아온 블록은 돌아온 때의 서버 시각을 받는다. 보드, 멤버, 카테고리의 `updatedAt`에는 이런 보장이 없다.
- 메시지는 보낸 순서대로 한 연결에 도착한다. 다른 쓰기의 메시지 사이의 순서는 쓰기가 끝난 순서와 다를 수 있다(메시지를 따로 돌며 보낸다).
- 서버는 64 KiB를 넘는 프레임과 메시지를 **받지** 않는다(Binary 프레임은 `collab_update_bytes`를 64 KiB 가까이나 그보다 크게 잡았으면 그 값에 32바이트를 더한 것까지 받는다). 서버가 보내는 메시지에는 한도가 없다(댓글이나 보기의 큰 `blockChanged`도, 문서 전체를 싣는 Binary 프레임도 그대로 간다).

## 5. 화면이 기대할 수 있는 것

실시간 변경을 쓰는 화면이 흔히 기대하는 것과, 서버가 하는 일이다. 메시지의 모양이나 뜻을 바꾸는 것은 사용자의 결정이다.

| # | 화면의 기대 | 서버 |
|---|---|---|
| G1 | 구독은 둘: 보드나 위키 하나(`watchContainer`), 나 자신(`watchSelf`) | 구독의 단위는 **워크스페이스**(`subscribeWorkspace`)와 **블록**(`subscribeBlocks`, 읽기 토큰으로)이다. 보드 하나만 구독하는 명령은 없다. 워크스페이스를 구독하면 내가 멤버인 모든 보드의 보드·블록·멤버 변경과 내 카테고리의 변경이 온다. 그러니 `watchContainer`와 `watchSelf`는 연결 하나의 워크스페이스 구독에서 화면이 보드 ID로 갈라 쓴다. 설정(`patchUserSettings`)의 변경은 메시지가 없다. 카테고리 메시지는 그 워크스페이스를 구독한 내 연결 모두에 간다(3절) |
| G2 | 변경의 종류 일곱 | 보드: `containerChanged`(전체). 보드 삭제: `deletedAt`이 있는 `containerChanged`(따로 된 종류가 없다). 블록: `blockChanged`이 **블록 하나씩**, `Block`을 싣는다. 카드·페이지·일기 항목·단원은 본문 대신 그 요약(`summary`, 3절)을 싣는다. 블록 삭제: `deletedAt`이 있는 `blockChanged`, 지운 블록 하나만(아래 것은 싣지 않는다). 구성원: 전체 목록이 아니라 멤버 한 명씩(`containerMemberChanged`, `containerMemberRemoved`). 카테고리: 전체 목록이 아니라 카테고리 하나(`categoryChanged`), 보드의 자리(`containerCategoriesChanged`), 순서(`categoriesReordered`, `categoryContainersReordered`). 설정: 메시지 없음 |
| G3 | 내 쓰기도 내게 온다 | 맞다. 보낸 연결도 받는다 |
| G4 | `updatedAt`이 블록마다 단조 증가 | **있는 블록을 고칠 때는 그렇다**: 고칠 때마다 앞의 값보다 커진다(4절). 휴지통에서 돌아온 블록은 그때의 서버 시각을 받으므로, 시계가 뒤로 갔다면 지우기 전보다 작을 수 있다. 판을 가르는 다른 값(카운터)은 없다 |
| G5 | 남이 본문을 고치면 카드의 요약이 온다 | **맞다.** 카드·페이지·일기 항목·단원의 `blockChanged`은 어느 길로 바뀌었든(방이 본문을 쓸 때, HTTP의 쓰기, 가져오기, 복제, 판 되돌리기, 휴지통에서 돌아옴) `body`가 `null`이고, 본문이 있는지와 할 일 수와 미리 보기를 `summary`에 싣는다(3절). 본문은 방이나 HTTP로 읽는다 |
| G6 | 재연결 때 놓친 변경은 화면이 보드를 다시 읽는다 | 맞다. 서버는 놓친 구간을 다시 보내지 않는다. 순번이 없어 무엇을 놓쳤는지도 알 수 없다 |


## 6. 본문의 공동 편집: 방

카드의 본문을 함께 고치는 연결들이 그 카드의 **방**에 든다. 위키의 페이지와 학습의 단원도 카드와 똑같이 방을 갖는다: 이 절의 "카드"는 페이지나 단원이기도 하고 그때 "보드"는 그 페이지의 위키나 단원의 학습이며, 멤버 `cardId`는 이름 그대로 카드의 ID, 페이지의 ID, 또는 단원의 ID를 싣는다(장의 블록은 방과 본문이 없다). 들고 쓰는 권한은 아래 "누가 드나"와 같이 위키·학습의 역할로 가르고, 본문이 페이지나 단원에 쓰이면 카드와 같이 `blockChanged`(`block`의 `type`이 `page`나 `unit`)이 나간다. 복제·템플릿·아카이브 내보내기와 블록 하나의 되살리기는 아직 위키와 페이지에 없다: 이 절에서 그것들을 말하는 대목은 카드와 보드의 것이고, 지운 위키와 학습은 `restoreContainer`로 그 블록들과 함께 돌아온다. 댓글의 본문에는 방이 없다. 서버는 본문을 Y 문서 하나로 쥐고(`docs/configuration.md`의 "본문의 공동 편집"), 방의 연결들과 그 문서의 변경을 주고받는다. 제어는 텍스트 메시지, 문서의 내용은 Binary 프레임이다. 구독(2–3절)과 같은 연결을 쓰고, 한 연결이 방 여럿에 들 수 있다. 본문이 카드에 쓰이면 본문 대신 요약을 실은 `blockChanged`(3절)이 나간다: 방에 든 연결도 워크스페이스를 구독했으면 그것을 함께 받는다.

### 들어오기와 나가기

`joinDoc`(`workspaceId`, `cardId`)에 서버는 하나로 답한다.

| 답 | 멤버 | 뜻 |
|---|---|---|
| `docJoined` | `workspaceId`, `cardId`, `doc`, `epoch`, `mode` | 방에 들었다. `doc`은 이 연결에서 그 방을 가리키는 번호(1부터, 들어올 때마다 새 번호)이고 뒤의 메시지와 프레임이 모두 이 번호로 방을 가리킨다. `epoch`은 문서의 세대(아래), `mode`는 `write`나 `read`. **그 방의 다음 Binary 프레임이 문서다**: 전체(sync step 2), 쥔 문서를 알리고 다시 든 연결에는 step 1과 차이(아래 "다시 들기"). 그 사이에 다른 방이나 구독의 메시지가 낄 수 있다 |
| `docRefused` | `workspaceId`, `cardId`, `reason` | 들지 못했다. `reason`은 아래 표 |

- **누가 드나.** 카드의 보드에서 카드를 고칠 수 있으면(편집 역할부터) `write`, 보드를 읽을 수만 있으면(보기·댓글 역할, 열린 템플릿과 참여하지 않은 열린 보드·열린 위키·열린 학습을 읽는 워크스페이스 구성원) `read`로 든다. 그 밖의 사용자, 세션 없는 연결(공유 보드를 읽기 토큰으로 따라가는 연결도), 비밀번호를 받지 않는 조직(`ssoRequired`)의 카드에 그 공급자로 만들지 않은 세션의 연결(HTTP의 401 `sso_required`에 해당한다), 비밀번호가 있는 계정의 내 공간의 카드에 공급자로 만들고 비밀번호를 확인하지(`confirmPassword`) 않은 세션의 연결(HTTP의 401 `password_sign_in_required`에 해당한다), 없는 카드, 방이 없는 블록(장·댓글·보드·위키·학습 자체의 ID도), `workspaceId`가 카드의 보드가 있는 워크스페이스가 아닌 것은 모두 `reason`이 `not_found`인 같은 `docRefused`다. `workspaceId`나 `cardId`가 64바이트를 넘는 `joinDoc`은 답 없이 버린다. 들 수 있는지는 카드가 어느 보드의 것인지만 읽어 가른다: 본문은 들인 뒤에 읽으므로 거절은 본문의 크기와 상관없이 같은 일을 한다.
- **클라이언트는 빈 문서로 든다.** 문서는 서버가 카드의 본문에서 만든다. 클라이언트가 본문에서 스스로 만든 문서는 서버의 것과 다른 문서여서 합치면 내용이 두 번 든다.
- 이미 든 카드에 다시 `joinDoc`을 보내면 새 번호로 새로 들고(문서를 다시 받는다) 앞의 번호는 더 방이 아니다(그 번호의 기다리던 Binary 프레임은 버려진다). 거절되면 앞의 것이 그대로다.
- **다시 들기.** 문서를 쥔 채 다시 드는 클라이언트(연결이 끊겼다 붙었다, 방에서 내보내졌다)는 `joinDoc`에 쥔 문서의 세대 `epoch`과 그 상태 벡터 `stateVector`(Yjs version 1, base64)를 싣는다. 방의 세대가 그 `epoch`이고 그 연결이 `write`로 들면 서버는 `docJoined` 뒤에 **step 1**(서버 문서의 상태 벡터)과 **step 2**(클라이언트의 상태 벡터에 없는 것)를 이 차례로 보낸다: 첫 프레임이 step 1이면 뒤따르는 step 2는 전체가 아니라 차이다. 클라이언트는 차이를 쥔 문서에 합치고, 서버가 받았다고 답하지 않은 자기 변경이 있으면 받은 상태 벡터에 없는 것(`encodeStateAsUpdate(문서, 그 상태 벡터)`: 문서의 지움은 모두 실린다)을 변경 하나로 보낸다(답은 `docAck`. 서버에 이미 있던 것이 섞여도 문서는 그대로다). 세대가 다르거나, `read`로 들거나(보낼 것이 없다), `stateVector`가 없거나 base64가 아니거나 상태 벡터로 읽히지 않거나, 그때 차이를 읽을 수 없으면(문서를 잠시 다룰 수 없다, 상태 벡터가 `collab_update_bytes`보다 크다) 서버는 전체(step 2 하나)를 보낸다: 다른 세대의 전체는 쥔 문서를 버리고 받고(아래 "세대"), 같은 세대의 전체는 쥔 문서에 합친다. 버리고 새로 받을 문서(`docNack` 뒤에 다시 드는 때. 문서를 그대로 쥐는 `document_unavailable`은 아니다)는 쥐었다고 알리지 않는다. 그런 연결이 앞서 보낸 `joinDoc`의 답으로 차이를 받으면 버리고 한 번 더 든다.
- `leaveDoc`(`doc`)으로 나온다. 연결이 끊겨도 나온다. 나온 뒤에도 그 번호로 이미 보낸 것이 조금 더 올 수 있다: 모르는 번호의 메시지와 프레임은 버린다.
- 마지막 연결이 나가면 서버가 문서를 닫으며 본문을 카드에 쓴다. 그 전에도 마지막 변경 2초 뒤(늦어도 첫 변경 10초 뒤)에 쓴다.

### Binary 프레임

양쪽이 같은 모양으로 보낸다. 수는 lib0의 varuint(바이트마다 7비트, 낮은 자리부터, 마지막이 아닌 바이트는 맨 윗비트가 1)다.

| 차례 | 무엇 |
|---|---|
| varuint `doc` | 어느 방인가(`docJoined`의 `doc`) |
| varuint 종류 | `0`: sync 메시지, `1`: 커서(아래 "커서"). 다른 값의 프레임은 버린다 |
| varuint `epoch` | 이 프레임이 문서의 어느 세대의 것인가 |
| varuint sync 종류 | sync 메시지에만 있다. `0`: step 1(상태 벡터), `1`: step 2(받는 쪽에 없는 것), `2`: update |
| varuint 길이, 그만큼의 바이트 | 상태 벡터(step 1)나 업데이트(step 2, update), Yjs의 version 1 부호화. 커서 프레임에서는 y-protocols의 awareness update. 프레임은 여기서 끝난다 |

sync 종류부터 끝까지는 y-protocols의 sync 메시지 그대로다(`writeSyncStep1`, `writeSyncStep2`, `writeUpdate`). 예: `doc` 3, `epoch` 300, 두 바이트 `aa bb`의 update는 `03 00 ac 02 02 02 aa bb`, 같은 두 바이트를 실은 커서 프레임은 `03 01 ac 02 02 aa bb`.

| 누가 | 프레임 | 서버가 하는 일 |
|---|---|---|
| 서버 | step 1 | 쥔 문서를 알리고 다시 든 연결에만, `docJoined` 뒤 그 방의 첫 프레임: 서버 문서의 상태 벡터("다시 들기") |
| 서버 | step 2 | `docJoined`와 `docReset` 뒤 그 방의 첫 프레임: 문서 전체(위의 step 1 뒤에서는 차이). step 1의 답: 그 상태 벡터에 없는 것 |
| 서버 | update | 다른 연결의 변경, 서버가 고친 것, 즉시 재확인 동안 받지 못한 것(아래) |
| 클라이언트 | step 1 | 그 연결에만 step 2로 답한다. 세대가 다르거나 상태 벡터가 아니면 답하지 않는다. `read`도 보낼 수 있다 |
| 클라이언트 | update, step 2 | 둘 다 변경으로 받는다(아래). 답은 `docAck`나 `docNack` |

서버가 step 1을 보내는 것은 그때뿐이다: 처음 드는 클라이언트는 서버에 없는 것을 갖고 들어오지 않는다(오프라인 편집이 없다). 이 모양이 아닌 프레임(끝나지 않는 수, 길이가 남은 바이트와 다름)과 들지 않은 방의 프레임은 답 없이 버린다.

### 커서

`write`로 든 연결은 본문에서 자기 커서(선택 범위의 두 끝)가 어디인지 알리고, 방의 다른 연결들이 그것을 받는다. 커서 프레임이 싣는 것은 y-protocols의 awareness update(`encodeAwarenessUpdate`)다: varuint 개수, 그리고 상태마다 varuint 번호, varuint 시계, varuint 길이와 그만큼의 UTF-8 JSON(사라진 상태는 `null`). 받는 쪽은 그대로 `applyAwarenessUpdate`에 넣는다. 답은 없다.

| 누가 | 싣는 것 | 서버가 하는 일 |
|---|---|---|
| 클라이언트 | 자기 상태 하나. 서버가 읽는 것은 `cursor`(`anchor`와 `head`, 각각 Yjs 상대 위치의 JSON: `type`, `tname`, `item`, `assoc`)뿐이다 | 방의 다른 연결들에 아래 모양으로 보낸다. `cursor`가 없거나 `null`이거나 상태가 `null`이면 그 연결의 커서를 거둔다. **번호, 시계, `user`, 그 밖에 상태에 적힌 것은 읽지 않는다** |
| 서버 | 커서 하나의 상태: `{"user":{"id":…},"cursor":{"anchor":…,"head":…}}`, 거둔 커서는 `null` | `user.id`는 **그 프레임을 보낸 연결의 사용자**다. 번호는 서버가 그 연결의 그 방 자리에 임의로 뽑아 준 수(2^32부터 2^52 미만: Yjs의 클라이언트 번호와 겹치지 않고, 한 방의 두 커서가 같은 수일 일은 없다고 보아도 된다. 몇 번째 커서인지는 드러내지 않는다), 시계는 그 커서를 알린 횟수다. 이름과 색은 싣지 않는다: 받는 쪽이 사용자 ID로 정한다 |

- **누구의 커서인지는 서버가 적는다.** 클라이언트가 남의 사용자 ID·이름·색이나 남의 번호를 실어 보내도 나가는 것은 보낸 연결의 사용자와 그 연결의 번호다. 한 연결은 방마다 커서 하나를 갖고, 같은 사람의 탭 여럿은 커서도 여럿이다.
- **누가 받나.** 그 방의 다른 연결(`read`도). 보낸 연결에는 돌아오지 않는다. 다른 방(다른 조직의 방도), 방에 들지 않은 연결(공유 보드의 독자)에는 가지 않는다. 즉시 재확인이 쥔 연결("방에 있는 동안")에는 그동안 가지 않고, 답이 그 연결을 남기면 그때의 커서들과 그사이 거둔 커서(64개까지. 나머지는 받는 쪽이 30초 뒤에 지운다)를 한 번에 받는다. 방에 드는 연결은 문서(전체나 차이) 뒤에 그때 보이는 커서들을 받는다. 둘 다 30초 넘게 소식이 없던 커서는 뺀다. 문서 전체를 다시 받는 연결(`docReset`)에 기다리던 그 세대의 커서는 그 전체 뒤로 간다.
- **버리는 것.** `read`로 든 연결의 커서 프레임, 방의 세대와 다른 `epoch`의 것, awareness update가 1,024바이트를 넘는 것, 상태가 하나가 아니거나 JSON 객체(또는 `null`)가 아닌 것, `cursor`가 정확히 `anchor`와 `head` 두 위치가 아닌 것. 위치가 아닌 것: 객체가 아니거나 모르는 멤버가 있다, `type`·`tname`·`item` 가운데 아무것도 없다, `tname`이 본문의 것(`default`)이 아니다, `type`·`item`의 `client`·`clock`이 0부터 2^53 미만의 정수가 아니다, `assoc`이 −128부터 127까지의 정수가 아니다. 버려도 방에 남고 알리지 않는다.
- **거두는 때.** 커서를 거뒀다고(상태 `null`, 그 커서의 번호와 다음 시계) 남은 연결들에 알리는 때: 그 연결이 거뒀을 때, 방을 나가거나(`leaveDoc`, 다시 듦) 내보내지거나(`docLeft`) 연결이 끊겼을 때, `docModeChanged`로 `read`가 됐을 때. 문서가 새 세대로 만들어지면 서버는 그 방의 커서를 모두 잊고 알리지 않는다: 받는 쪽은 버리는 문서와 함께 버린다.
- **한도.** 커서 프레임도 `collab_frames_per_second`에 하나로 센다. 넘은 커서 프레임은 버릴 뿐 방에서 내보내지 않는다(문서와 어긋나지 않는다. 프레임의 모양이 아닌 것은 여느 프레임처럼 내보낸다). 받는 연결의 줄에는 커서마다 마지막 알림 하나만 기다리고(새 알림이 기다리던 것의 자리를 갈아 든다. 거둠도), 기다리는 것이 줄(`collab_queue_messages`, 바이트도)의 절반이 되면 기다리는 알림이 없는 커서는 넣지 않는다: 커서만으로는 줄이 차지 않고, 커서 때문에 내보내지도 않는다. 클라이언트는 커서가 그대로여도 15초마다 다시 알리고(y-protocols가 그렇게 한다), 30초 동안 소식이 없는 커서는 받는 쪽이 지운다.
- 커서는 서버의 메모리에만 있다. DB에도 로그에도 쓰지 않는다.

### 변경의 순서와 답

| 메시지 | 멤버 | 뜻 |
|---|---|---|
| `docAck` | `doc`, `epoch`, `seq` | 보낸 변경을 받아들였다. `seq`는 문서의 기록에서 그 변경의 순번 |
| `docNack` | `doc`, `epoch`, `reason` | 보낸 변경을 받아들이지 않았다. 문서는 그대로이고 누구에게도 가지 않았다 |
| `docReset` | `doc`, `epoch` | 문서를 다시 보낸다. **그 방의 다음 Binary 프레임이 문서 전체다**(step 2. 세대, 아래) |
| `docModeChanged` | `doc`, `mode` | 이 연결이 이제부터 `write`나 `read`다(아래 "방에 있는 동안") |
| `docLeft` | `doc`, `reason` | 서버가 이 연결을 방에서 내보냈다. 그 번호는 더 방이 아니다. 그 방에서 이미 정해진 답(`docJoined`, `docAck`, `docNack`, `docModeChanged`)은 이보다 먼저 오고, 기다리던 그 방의 Binary 프레임은 버려진다. 보낼 줄이 차서 내보내는 때에도 그렇다: 기록에 적힌 변경의 `docAck`는 줄이 찼어도 실리고 그 뒤에 `docLeft`가 온다 |

- **기록이 먼저다.** 변경은 서버가 문서에 넣고 기록(DB)에 적은 뒤에만 나간다: 보낸 연결에 `docAck`, 방의 다른 연결에 update 프레임. 보낸 연결에는 자기 변경이 돌아오지 않는다. 방의 연결들은 변경을 기록의 순서대로 받는다.
- **변경 프레임 하나에 답 하나.** 방에 든 연결이 보낸 update·step 2 프레임마다 `docAck`나 `docNack`가 보낸 순서대로 하나씩 온다. `epoch`은 그 프레임이 실은 값 그대로다. `docLeft`를 받은 방의 것은 더 오지 않는다.
- **서버가 고친 것.** 받아들인 변경이 문서를 본문이 아닌 것으로 만들면 서버가 그만큼 지우고, 그 지움을 update 프레임으로 **보낸 연결을 포함한 모두에게** 보낸다(`docAck` 뒤에).
- **`docNack` 뒤.** 받아들여지지 않은 변경은 보낸 편집기의 문서에만 남아 있다. 그 편집기는 방에 **한 번** 다시 들어(`joinDoc`) 문서를 새로 받는다(`docNack`마다가 아니다: `joinDoc`은 프레임 다섯으로 센다). 그대로 두면 그 위에 얹은 다음 변경도 `update_out_of_step`으로 거절된다. 다시 들지 않아도 되는 때: `epoch`이 이미 받은 `docReset`의 것보다 옛것인 `docNack`는 버린 문서의 답이므로 무시하고, 쥐고 있는 `epoch`의 `update_out_of_step`은 문서가 방금 새로 만들어진 것일 수 있으니 곧 올 `docReset`을 잠깐 기다린 뒤에 든다.

### 방에 있는 동안

구성원·역할·보드 공개 범위·조직 관리자 참여·워크스페이스의 사용 여부·팀이 바뀌는 조작은 답하기 전에 해당 방들의 권한을 다시 묻는다. 팀의 조작(`addTeamMember`, `removeTeamMember`, `deleteTeam`, `setContainerTeam`, `removeContainerTeam`)은 그 팀이 걸린 보드들에서 그 팀의 사람만(사람을 넣고 빼면 그 사람만, 보드의 팀 역할을 바꾸면 그 보드에서) 묻고(이미 그 역할이거나 뗄 역할이 없거나, 이미 팀에 있는 사람을 넣거나 팀에 없는 사람을 빼는 조작은 아무도 묻지 않는다), `restoreContainer`는 사람과 남은 팀의 역할이 다시 걸린 그 보드를 묻는다. 한 사람의 사실이 바뀐 조작(구성원·역할, 참여)은 그 사람의 연결만(워크스페이스에서 빠지거나 손님이 된 사람이 혼자 관리하던 보드가 그 조작을 한 관리자에게 넘어가면 그 보드에서는 그 관리자도), 보드나 워크스페이스 전체가 바뀐 조작은 모두를 묻고, 같은 보드의 같은 사용자·세션 방식은 여러 카드 방에 있어도 한 번만 묻고, 여럿은 여덟까지 함께 묻는다. 보드 여럿을 한꺼번에 바꾸는 조작(묶음, 가져오기)은 보드마다가 아니라 한 번에 다시 묻는다. 다른 조직의 조작은 기다리지도 다시 묻게 하지도 않는다. 서버는 그와 별개로 설정 `collab_recheck_seconds`(기본 30초)마다 방의 연결마다 들어올 때와 같은 것을 다시 묻는다(같은 사용자가 같은 방식(비밀번호, 어느 공급자)으로 만든 세션의 연결들은 한 번에).

- 더 읽을 수 없으면(보드에서 빠졌다, 워크스페이스에서 빠졌다, 카드가 지워졌거나 다른 워크스페이스의 보드로 갔다, 조직이 비밀번호를 받지 않게 됐는데(`ssoRequired`) 연결의 세션이 그 공급자로 만든 것이 아니다) `reason`이 `not_found`인 `docLeft`로 내보낸다. 물을 수 없었을 때(저장소의 오류, 한 간격 안에 답이 없음)는 2초 뒤에 한 번 더 묻고, 그래도 답이 없으면 그 오류의 `reason`(답이 없었으면 `document_unavailable`)으로 내보낸다: 확인되지 않은 연결은 방에 남지 않는다.
- 쓰던 연결이 읽을 수만 있게 됐으면 `docModeChanged`(`read`)를 보내고 그 뒤의 변경은 `docNack`(`document_read_only`)이다. 반대로 쓸 수 있게 됐으면 `docModeChanged`(`write`)다.
- 즉시 재확인은 방을 쥐지 않고 저장소에 묻는다. 묻는 동안 기다리는 것은 그 조작이 해당하는 연결뿐이다: 그 연결이 보낸 변경은 답이 적용되기 전에는 받아들여지지 않고, 연결이 그 뒤에 보낸 것(다른 방의 것도)은 그 뒤에 처리된다. 방의 다른 연결, 다른 방의 연결, 같은 조직의 다른 조작은 기다리지 않는다. 묻는 동안 그 연결에는 그 방의 Binary 프레임과 `docReset`이 가지 않는다(다른 연결의 변경, 서버가 고친 것, 다시 보내는 문서 전체, step 1의 답). 답이 그 연결을 남기면 그동안 받지 못한 것을 한 번에 보낸다(`docModeChanged`가 있으면 그 뒤에): 문서가 그대로면 그 연결에 없는 것을 update 프레임 하나로, 그 사이 문서가 새로 만들어졌거나 그 차이를 읽을 수 없으면 `docReset`과 문서 전체로. 그것도 열 수 없으면 그 까닭의 `reason`으로 내보낸다. 답이 그 연결을 내보내면 `docLeft` 말고는 아무것도 보내지 않는다. 묻기 시작할 때 이미 진행 중이던 변경은 조작의 답보다 먼저 끝난다. 그 사이에 같은 사람에 대한 더 새 조작이 오면 앞 재확인의 답은 버려지고 새 조작의 답이 적용된다. 그 전에 들어오던 연결은 다시 묻고, 주기 재확인의 오래된 답은 즉시 재확인의 결과를 덮지 않는다. 둘 다 그 방과 그 사람에 닿는 조작만 본다: 다른 보드나 다른 사람의 조작은 들어오던 연결을 다시 묻게 하지도 주기 재확인의 답을 버리게 하지도 않는다. 저장소가 즉시 재확인에 답하지 못하면 그 오류의 `reason`으로 내보낸다. 조회 기한은 읽기마다 한 간격(느린 읽기 하나가 다른 사람의 읽기 시간을 쓰지 않는다), 재확인 전체로 두 간격이며, 기한까지 답이 없으면 그 사람만 `document_unavailable`로 내보낸다(기다리던 변경은 답 없이 버려진다). 또한 조직이 비밀번호를 받지 않게 되면 그 공급자로 만들지 않은 세션의 연결은 그 조직의 방에서 저장소에 묻지 않고 곧바로 `not_found`의 `docLeft`로 나간다(그것을 켠 요청은 그 뒤에 답한다). 그 전에 들어오던 연결은 다시 물은 뒤에 든다. 세션이 끝난 연결은 연결째 닫힌다(3절). 방에서 나간 연결은 연결과 그 연결의 다른 방·구독이 그대로 남는다.

카드나 보드를 지우기 전에는 방의 마지막 변경까지 카드 본문에 쓴다. 쓰지 못해도 삭제는 이루어지고, 쓰지 못한 변경은 기록에 남아 되살린 카드를 열 때 반영된다. 삭제와 그 조직의 문서 작업은 엇갈리지 않는다. 삭제된 방의 모두는 `docLeft`(`not_found`)로 나가며, 그 방의 뒤늦은 입력은 카드를 되살린 뒤에도 받아들이지 않는다. 되살린 카드의 새 방은 삭제 때 저장한 본문에서 열린다.

카드·보드 복제, 템플릿 만들기, 아카이브 내보내기는 방의 문서에 있고 카드 본문에는 아직 없는 변경을 카드에 먼저 쓴 뒤 읽는다. 열린 문서가 없더라도 남은 기록은 같은 순서로 반영한다: 그 기록만 쓰려고 여는 문서는 서버가 쥘 수 있는 문서의 수와 따로 세므로(`docs/configuration.md`의 `collab_documents_per_worker`), 서버의 문서가 다 찼어도 그 때문에 실패하지 않는다. 먼저 쓰기에 실패하면 복제나 내보내기도 실패한다.

### 세대

`epoch`은 문서가 몇 번째로 만들어진 것인가다(1부터). 서버는 문서가 한도 가까이 자랐을 때(크기, 클라이언트 수, 한 클라이언트의 변경 수, 세션 하나가 더 쓰면 읽는 데 들 수 있는 걸음), 마지막 연결이 방을 나가 문서를 닫을 때 여러 세션이 쓴 문서이면(클라이언트 수나 그 걸음이 한도의 8분의 1: `docs/configuration.md`의 `collab_update_bytes`. 본문이 카드에 쓰인 뒤이고 문서를 쥔 연결이 없으므로 `docReset`은 가지 않는다. 끊겼다가 쥔 문서를 알리며 다시 드는 연결은 다른 세대의 전체를 받는다: "다시 들기"), 받아들이면 읽는 데 드는 걸음이 한도를 넘는 변경을 받아들인 직후(본문에 쓰고 새 문서로 만든 뒤 `docAck`에 이어 `docReset`. 그 변경도 본문과 새 문서에 든다. 다만 고칠 것이 함께 있는 변경만 `document_too_large`로 거절하고 거절한 변경 없이 새로 만든다)와 카드의 본문이 문서를 거치지 않고 통째로 쓰였을 때(HTTP의 `patchCard`·`patchPage`·`patchUnit` 등, 판으로 되돌리기 `restoreCardBodyVersion`·`restorePageBodyVersion`·`restoreUnitBodyVersion`, 가져오기, 되살리기) 카드의 본문에서 문서를 **새로 만든다**. 내용이 같아도 다른 문서다: 앞 세대의 변경은 새 문서에 얹히지 않는다.

- 서버는 프레임마다 `epoch`을 싣고, 클라이언트도 자기가 쥔 세대를 싣는다. 방의 세대와 다른 변경은 `docNack`(`update_out_of_step`)이다.
- 문서가 새로 만들어지면 방의 모두에게 새 `epoch`의 `docReset`과 문서 전체가 간다. 클라이언트는 **쥐고 있던 문서를 버리고** 온 것으로 새 문서를 만든다. 아직 답을 받지 못한 변경은 버려진 문서의 것이다(늦게 오는 그 답은 옛 `epoch`을 싣고 있다). 자기 `epoch`과 다른 프레임은 버린다. 새로 만들어진 때와 `docReset` 사이에 보낸 변경은 `docNack`(`update_out_of_step`)이다.
- `epoch`이 그대로인 `docReset`은 같은 문서를 다시 보낸 것이다(문서를 쥔 작업자 프로세스가 죽어 서버가 저장된 것에서 다시 열었을 때 등. 까닭은 싣지 않는다). 뒤따르는 전체를 쥔 문서에 그대로 합친다(버리지 않는다). 받아들여진 변경은 모두 그 안에 있고, 그 `epoch`의 변경은 앞뒤로 그대로 받아들여진다. 작업자가 어떤 변경을 받다가 죽은 것이면 그 변경은 문서에 없고 방에는 아무것도 가지 않으며, 그 변경을 보낸 연결은 닫힌다(3절의 표. 그 전에 `docNack`(`document_unavailable`)이 갈 수도 있다).
- 방에 문서를 다시 보내려는데 열 수 없으면: 지금 다룰 수 없는 것(`document_unavailable`)이면 방은 그대로 두고 2초마다 다시 해 본다(그동안의 변경은 그 `reason`으로 거절될 수 있다). 그 밖의 까닭(서버가 문서를 더 쥘 수 없다, 카드가 없다)이면 방의 모두가 그 `reason`의 `docLeft`를 받는다. 카드를 지우면 방의 모두가 `docLeft`(`not_found`)로 나간다("방에 있는 동안").

### `reason`의 값

값은 HTTP의 문제 이름과 같다: `reason`이 있는 문제는 그 값, 없는 문제는 `type`의 끝(`/problems/not-found`)을 밑줄로 적은 것(`not_found`). 설명은 `/problems/<이름>`에 있다.

| `reason` | 어디서 | 뜻 |
|---|---|---|
| `not_found` | `docRefused`, `docNack`, `docLeft` | 그 카드가 이 사용자에게 없다(위 "누가 드나"). 방에 든 뒤에는 카드가 없어졌거나 이 사용자가 더 읽을 수 없게 된 것이다("방에 있는 동안") |
| `update_refused` | `docNack` | 업데이트가 아니거나, 본문 밖을 쓰거나, 문서를 본문이 아닌 것으로 만들거나, 보드에 없는 파일의 이미지를 보인다 |
| `update_out_of_step` | `docNack` | 방의 세대와 다른 `epoch`, 또는 문서에 없는 변경 위에 얹은 변경 |
| `update_too_large` | `docNack` | 업데이트가 `collab_update_bytes`를 넘는다 |
| `document_too_large` | `docNack` | 받아들이면 문서나 본문이 한도를 넘는다(크기, 그리고 문서를 읽는 데 드는 걸음: `docs/configuration.md`의 `collab_update_bytes`) |
| `table_too_large` | `docNack` | 받아들이면 표가 한도(200행, 20열)를 넘는다 |
| `document_read_only` | `docNack` | `read`로 든 연결의 변경 |
| `too_many_documents` | `docRefused`, `docNack`, `docLeft` | 서버가 쥘 수 있는 문서가 다 찼다 |
| `document_unavailable` | `docRefused`, `docNack`, `docLeft` | 지금 그 문서를 다룰 수 없다(작업자가 죽었거나 답하지 않았다, 문서가 잠시 닫혔다, 서버가 멈추는 중이다). 그 변경은 문서에 없다 |
| `too_many_documents_joined` | `docRefused` | 이 연결이 이미 `collab_documents_per_connection`(기본 8)개의 방에 들어 있다. 이미 든 카드에 다시 드는 것은 세지 않는다 |
| `too_many_connections_joined` | `docRefused` | 그 방에 이 사용자의 연결이 이미 `collab_connections_per_user`(기본 5)개 들어 있다(같은 카드를 연 탭과 기기). 이미 든 연결이 다시 드는 것은 세지 않는다 |
| `document_full` | `docRefused` | 그 방에 이미 `collab_connections_per_document`(기본 50)개의 연결이 들어 있다 |
| `too_many_frames` | `docRefused`, `docLeft` | 이 연결이 `collab_frames_per_second`(기본 50)보다 빨리 보냈다. 쉬고 난 뒤에는 그만큼을 한꺼번에, 그 뒤로 1초에 그만큼씩 보낼 수 있다. Binary 프레임은 모두 하나로(들지 않은 방의 것도, 프레임이 아닌 것도), 문서 전체로 답하는 `joinDoc`과 step 1은 다섯으로, `leaveDoc`은 하나로 센다(나가기는 넘어도 된다). 서버가 이 연결의 일을 하느라 읽지 못한 시간의 몫은 그대로 받는다. 넘은 `joinDoc`은 거절되고, 넘은 프레임은 그 프레임의 방에서 이 연결을 내보낸다: 그 프레임이 실은 것을 방이 받지 않았으므로 다시 들어 문서를 새로 받는다. 커서 프레임만은 넘어도 버리기만 한다 |
| `connection_too_slow` | `docLeft`, `docRefused` | 이 연결에 보낼 방의 메시지가 `collab_queue_messages`(기본 512)개, 또는 `collab_state_bytes`의 두 배(바이트)를 넘게 쌓였다(읽지 않는 연결. step 1을 답을 읽지 않고 거듭 보내도 그렇다). 방의 다른 연결은 그대로 받는다. 다시 들면 문서 전체를 받는다 |
| `internal` 등 | 모두 | 서버의 오류(저장소 등). 로그에 남는다. 저장소가 제때 받지 못한 것은 `database_busy`다(잠시 뒤 다시) |


## 7. 보드의 있음

`setPresence`는 `workspaceId`, `containerId`, `cardId`(열지 않았으면 빈 문자열),
`readToken`(구성원은 생략)을 받는다. `containerId`가 비면 떠난다. 답은 없다.
연결당 한 보드·한 카드이며 새 명령이 앞의 것을 바꾼다. 보드의 읽기 권한은
`App::container_access`, 세션의 조직 접근은 `enter_workspace_as`로 확인한다.
카드가 같은 보드의 지우지 않은 카드가 아니면 카드 없이 보드에 있는 것으로 받고, 연
카드가 나중에 지워져도 카드 없이 남는다. 위키와 학습도 같다: `containerId`에 위키나 학습의 ID를, `cardId`에
그 위키의 페이지나 학습의 단원 ID를 싣고(장의 ID는 없는 것으로 본다), 명단의 `cardIds`가 연 페이지·단원들이다. 위키나 학습의 읽기 토큰으로도
보드와 같이 익명으로 방문한다(블록 구독은 보드의 토큰만 받는다). 식별자는 방과 같은 최대 64바이트다.
명령은 `collab_frames_per_second`의 속도와 한 초의 버스트로 제한하고, 떠나기는
항상 받는다. 읽기 토큰은 현재 서버의 43글자 비밀 값이며 모양이 틀린 명령은 무시한다.
보드를 읽을 수 없어 거절된 명령은 그 연결의 앞선 방문도 끝낸다.

연결마다 250ms 간격으로 변경을 모아 `presenceChanged`의 전체 명단을 보낸다:
`workspaceId`, `containerId`, `people: [{userId, cardIds}]`, `anonymousCount`, `hiddenCount`.
사람과 카드 ID는 오름차순이며 같은 사람의 탭은 하나로, 카드는 합집합으로 센다.
사용자 조회에서 보이지 않는 사람은 `hiddenCount`로만 센다. 읽기 토큰을 명시한
연결은 세션이 있어도 익명 하나로 세고 명단은 받지 않는다. 익명은 연결별 수다.
공개 링크로 그 보드를 읽는 연결의 한도(3절의 `public link connections full`)를 넘는
읽기 토큰의 방문은 받지 않아 익명 수에 들지 않는다(앞선 방문도 끝난다).
같은 명단은 다시 보내지 않는다. 받는 사람의 권한과 누구를 볼 수 있는지는 보드에
들어올 때, `collab_recheck_seconds`마다, 아래의 권한 변경 때 저장소에 다시 묻고,
읽을 수 없으면 표에서 뺀다. 그 사이 명단이 바뀌면 처음 만나는 사람만 묻는다.
권한이 바뀌는 조작(6절의 즉시 재확인과 같은 것들)이 보드나 카드에 닿고, 그 보드에 와
있는 사람(받는 사람 자신이나 명단의 사람)에 관한 것이면 그 조작이 닿은 연결은 다음
묶음에서 곧바로 다시 본다. 조회하는 동안 바뀌었으면 그 명단은 보내지 않고 다시 묻는다.
이 보드에 닿지 않은 변경(다른 보드를 함께 쓰는 것으로만 보이던 사람이 그 보드에서
빠졌다 등)은 주기 재확인까지 앞서 본 대로다.
확인 실패·5초의 확인 시간 초과도 방문을 끝낸다. 다음 명령으로 다시 방문할 수 있다.

클라이언트는 30초마다 방문을 갱신한다. 90초간 갱신이 없으면 연결이 닫히지 않아도
빠진다. 연결 종료·쓰기 실패·서버의 연결 작업 취소에서도 빠진다. 서버 메모리에만
두며 DB에는 쓰지 않는다. 웹의 가려진 탭은 1분 뒤 떠나고 다시 보이면 돌아온다.
