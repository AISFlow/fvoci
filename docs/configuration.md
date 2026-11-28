# 서버의 설정

적은 때: 2026-11-11 05:05 KST, 고친 때: 2026-11-28 03:40 KST. 서버(`fvoci-server`)가 읽는 설정의 전부다. 코드는 `crates/server/src/config.rs`다.

키를 더하거나 바꾸면 같은 커밋에서 이 문서를 고친다. 이 문서는 코드와 함께 검사된다. `config.rs`의 테스트 `the_configuration_document_lists_every_key`가 다음을 견준다.

- 코드가 받는 키와 아래 "키" 표들에 적힌 키가 같은지.
- 키마다 기본값이 같은지.
- 환경 변수로 바꿀 수 있는 키가 같은지.

## 읽는 법

설정은 세 곳에서 온다. 뒤의 것이 앞의 것을 이긴다.

1. **설정 파일.** JSON 객체 하나다. 자리는 명령줄의 `--config`로 주며 기본은 `./config.json`이다.
   - 키는 아래 표에 적힌 그대로일 때만 맞는다(`server_url`과 `Server_URL`은 다른 키다). `file_storage_s3` 안의 키도 같다. 같은 키를 두 번 주면(모르는 키도) 서버가 시작하지 않는다. 오류는 키 이름과 그 자리만 말하고 값은 말하지 않는다. `file_storage_s3` 밖의 객체(`feature_flags` 등) 안은 보지 않는다.
   - 읽지 않는 키(모르는 키, 글자가 다른 키)가 있으면 서버가 시작할 때 그 이름들을 경고 한 줄로 로그에 적는다(`file_storage_s3` 안의 것은 `file_storage_s3.<키>`). 값은 적지 않는다. 시작은 막지 않는다.
   - 값의 타입이 틀리면 서버가 시작하지 않는다(`file_storage_s3`가 객체도 `null`도 아닌 것도).
2. **환경 변수.** `FVOCI_`에 키를 대문자로 이은 이름이다(예: `FVOCI_SESSION_IDLE_DAYS`. `file_storage_s3` 안의 키는 점을 `_`로 써서 `FVOCI_FILE_STORAGE_S3_BUCKET`). 아래 표의 "환경 변수" 칸에 이름이 있는 키만 바꿀 수 있다. 그 밖의 `FVOCI_` 변수는 읽지도 경고하지도 않는다.
   - 수는 정수로 읽는다.
   - 참·거짓은 `true`·`1`·`t`·`yes`·`on`과 `false`·`0`·`f`·`no`·`off`로 읽는다.
   - 목록은 쉼표로 나눈다.
3. **명령줄 인자.** `--port`, `--database-kind`, `--database-connection` 셋이다(아래 "명령줄 인자"). 빈 값이나 0은 무시한다.

읽은 뒤에 서버는 값을 검사하고, 받을 수 없는 값이 있으면 시작하지 않는다. 받는 범위는 표의 "타입" 칸과 "뜻과 범위" 칸에 있다.

"기본값" 칸은 JSON으로 적는다. 키를 빼면 그 값이다.

## 키: 서버의 주소와 화면

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `server_url` | `FVOCI_SERVER_URL` | 문자열 | `"http://localhost:8000"` | 사용자가 서버에 닿는 주소다. 경로가 있으면 웹앱이 그 경로를 자기 주소의 기준으로 쓴다. 서버가 실제로 그 경로 아래에서 답하게 하는 것은 `FVOCI_BASE_PATH`(아래 "그 밖의 환경 변수")이므로 둘을 같게 준다. 경로를 떼어 넘기는 프록시 뒤라면 그 변수는 비워 둔다. 출처 루트에서만 읽히는 `/robots.txt`(`Disallow: /`)와 `/.well-known/security.txt`는 `FVOCI_BASE_PATH`가 있어도 언제나 서버의 출처 루트에서 답한다(다른 앱과 한 출처를 나누는 프록시 뒤라면 프록시가 출처 루트를 맡고, fvoci가 내주는 페이지의 `X-Robots-Tag: noindex, nofollow`가 색인을 막는다). 바꾸는 요청과 WebSocket이 받아들이는 출처(`Origin`)가 이 주소에서 나온다. `https://`로 시작하면 세션 쿠키가 `Secure`가 되고 이름이 `__Host-fvoci_session`이 된다. 출처로 읽을 수 없는 주소면 시작하지 않는다 |
| `port` | `FVOCI_PORT` | 정수(1–65535) | `8000` | 들을 포트. 0이면 시작하지 않는다 |
| `listen_loopback_only` | `FVOCI_LISTEN_LOOPBACK_ONLY` | 참·거짓 | `false` | 참이면 루프백(`localhost`)에서만 듣는다. 거짓이면 모든 인터페이스(IPv6, 안 되면 IPv4)에서 듣는다 |
| `serve_https` | `FVOCI_SERVE_HTTPS` | 참·거짓 | `false` | 참이면 서버가 직접 HTTPS를 연다. 인증서는 작업 디렉터리의 `./cert/cert.pem`과 `./cert/key.pem`이다 |
| `web_app_path` | `FVOCI_WEB_APP_PATH` | 문자열 | `"./pack"` | 웹앱의 빌드 산출물이 든 디렉터리(`index.html`과 `static/`) |
| `allowed_origins` | `FVOCI_ALLOWED_ORIGINS` | 문자열 목록 | `[]` | `server_url`의 출처 말고도 바꾸는 요청과 WebSocket을 받아들일 출처. 개발할 때 웹앱을 다른 포트에서 띄우는 경우에 쓴다. 출처로 읽을 수 없는 값이 있으면 시작하지 않는다 |
| `websocket_ping_seconds` | `FVOCI_WEBSOCKET_PING_SECONDS` | 정수(1–59) | `30` | WebSocket마다 Ping을 보내는 간격(초). 한가한 연결을 프록시가 끊지 않게 한다. Ping마다 연결의 세션도 다시 보고 끝났으면 닫는다. 브라우저의 Pong은 연결이 보낸 것으로 센다: 90초 동안 아무것도 보내지 않은 연결은 닫는다(설정이 아니다. `contracts/websocket.md` 3절의 "죽은 연결"). 타이머가 멈춘 페이지도 느린 망에서 그 안에 Pong을 돌려보내도록 30초를 남겨 59초까지만 받는다. 0이거나 60 이상이면 시작하지 않는다 |
| `public_link_connections_per_container` | `FVOCI_PUBLIC_LINK_CONNECTIONS_PER_CONTAINER` | 정수(1 이상) | `200` | 공개 링크로 한 보드나 위키를 함께 읽는 연결의 수. 링크의 읽기 토큰으로 블록을 따라가거나(`subscribeBlocks`) 그 토큰으로 방문을 알리는(`setPresence`) 연결을 하나로 센다(로그인한 사람의 연결도 링크로 읽으면 센다). 다 찼으면 새 연결의 구독은 받지 않고 `blocksUnsubscribed`(`reason` `public link connections full`)로 알리며, 방문은 받지 않아 보드의 "익명 n명"에 세지 않는다. 이미 읽는 연결과 보드의 구성원이 보통으로 여는 연결에는 닿지 않는다. 0은 시작 오류다 |
| `public_link_connections_per_address` | `FVOCI_PUBLIC_LINK_CONNECTIONS_PER_ADDRESS` | 정수(1 이상) | `10` | 위의 연결 가운데 한 주소에서 오는 것의 수. IPv6 주소는 /64로 센다. 주소는 요청 로그와 로그인 제한이 쓰는 것이다(`trusted_proxies` 뒤면 `X-Forwarded-For`). 넘으면 위와 같다. 0은 시작 오류다 |
| `compress_responses` | `FVOCI_COMPRESS_RESPONSES` | 참·거짓 | `true` | 참이면 API의 JSON 응답을 클라이언트의 `Accept-Encoding`에 따라 zstd, brotli, gzip으로 압축한다(32바이트 미만과 오류 응답은 그대로). 압축하는 프록시나 CDN 뒤라면 거짓으로 둔다. 내보내는 아카이브와 파일은 이것과 관계없이 압축하지 않는다(길이를 그대로 보낸다). 웹앱의 정적 파일은 빌드가 만든 `.zst`, `.br`, `.gz`를 고르므로 이것과 관계없다 |
| `security_contact` | `FVOCI_SECURITY_CONTACT` | 문자열 | `""` | `GET /.well-known/security.txt`(RFC 9116)의 `Contact`에 실을 보안 제보 연락처(`mailto:security@example.com`, `https://example.com/security`, `tel:+1-555-0100` 꼴). 비어 있으면 `/.well-known/security.txt`는 404다. 값이 있으면 `Expires`는 응답 시각 기준 180일 뒤(UTC RFC 3339)로 내고, `server_url`이 `https://`이면 출처 기준 `Canonical`을 함께 낸다. 옛 경로 `/security.txt`는 언제나 404다. 위 세 스킴의 URI가 아니거나 공백·제어 문자가 있으면 시작하지 않는다 |

## 키: 데이터베이스

한 데이터베이스는 서버 하나가 쓴다. 서버 여럿이 한 데이터베이스를 나눠 쓰는 배치는 지원하지 않는다: WebSocket의 알림, 로그인 제한, 열린 업로드의 한도가 서버의 메모리에서 셈을 하고, 끝나지 않은 업로드의 부분은 그 서버의 `tmp_path`에 있다. 서버는 시작할 때, 스키마를 고치거나 남은 것을 치우기 전에 그 데이터베이스의 "서버 잠금"을 쥔다. PostgreSQL은 풀 밖의 전용 연결이 쥐는 세션 advisory lock이고(키는 표가 든 스키마의 해시. 다른 스키마의 서버끼리는 막지 않는다), SQLite는 DB 파일 곁의 `<DB 파일>-server.lock`의 파일 잠금이다(심볼릭 링크는 따라가 실제 파일 곁에 둔다. 하드 링크는 다른 이름으로 치므로 한 DB에 하드 링크 둘로 서버 둘을 띄우지 않는다. 메모리의 DB는 잠그지 않는다). 다른 프로세스가 쥐고 있으면 `server_lock_wait_seconds`만큼 0.25초마다 다시 해 보다가(DB에 잠깐 닿지 못한 것도 다시 해 본다), 누가 쥐고 있는지(PostgreSQL은 그 연결의 `application_name`, 백엔드 pid, 연결된 때. SQLite는 잠금 파일에 적힌 것. 둘 다 `fvoci-server process <pid>`) 말하고 시작을 거절한다. 서버는 잠금을 5초마다 보고, 잃으면(전용 연결이 끊김, 10초 안에 답이 없음) 배경 작업을 멈추고 준비 상태(`GET /api/v1/ready`의 `database`)를 거짓으로 하고 내려간다. 잃은 것을 알기까지 최대 15초와 요청을 마저 보내는 10초 동안은 그 서버의 요청이 아직 돌 수 있다: advisory lock은 그 틈을 막지 못한다. 다만 잃은 것을 안 뒤로는 객체 하나를 지우기 직전마다 잠금을 다시 보므로 더 지우지 않는다. 끝낼 때는 DB를 닫은 뒤 잠금을 놓는다. 5초 안에 닫히지 않으면(빌려 간 연결의 트랜잭션이 아직 돌면) 잠금을 스스로 놓지 않는다: 서버 프로세스는 곧 끝나고, 잠금은 그 트랜잭션의 연결과 함께 끊긴다(같은 프로세스에서 계속 돈다면 연결이 다 돌아온 뒤에 놓는다). 그래서 대개 재시작하는 서버는 앞선 서버가 내려가는 대로 0.25초 안에 잠금을 얻는다. 앞선 서버의 기기가 죽어 연결이 끊긴 것을 모르는 때는 PostgreSQL이 그 연결을 TCP keepalive(10초 뒤부터 5초마다 세 번)로 30초 안팎에 끊는다. 서버가 붙는 역할은 표의 주인이 아니어도 된다: 표의 SELECT·INSERT·UPDATE·DELETE와 시퀀스의 USAGE·SELECT만 가진 역할은 기록된 마이그레이션이 이 서버의 것과 같으면 스키마를 고치는 문장 없이 열고(그 역할이 읽고 쓰지 못하는 표가 있으면 그 표를 말하며 시작하지 않는다), 다르면(밀린 마이그레이션, 빈 스키마) 시작하지 않고 표의 주인으로 `fvoci-server --migrate`를 먼저 돌리라고 말한다. 권한은 마이그레이션하는 역할이 앞으로 만들 표에도 가게 첫 `--migrate` 전에 `ALTER DEFAULT PRIVILEGES FOR ROLE <그 역할> IN SCHEMA <스키마> GRANT …`로 준다. 두 역할의 `database_connection`은 같은 스키마를 가리켜야 한다(`search_path`를 주면 둘 다에): 서버 잠금의 키도 그 스키마다. 주인은 `ALTER TABLE`로 행 단위 보안을 끄거나 정책을 지울 수 있지만 주인이 아닌 역할은 그러지 못하므로, 서버가 뚫려도 조직 사이의 벽은 남는다. 서버의 역할이 표의 주인이거나 슈퍼유저면(역할 하나로 쓰는 설치, SQLite) 지금처럼 시작할 때 마이그레이션한다. 주인 역할의 구성원일 뿐인 역할은 주인으로 치지 않는다(그 역할이 만든 표는 그 역할의 것이 된다). 잠금의 연결은 DB 서버의 세션 하나를 혼자 써야 한다: PgBouncer 같은 풀러는 세션 모드로만 거친다(트랜잭션 모드는 잠금이 다른 클라이언트에게 넘어간다). 새 서버가 준비된 뒤에 옛 서버를 내리는 배포(Kubernetes의 기본 rolling update)는 새 서버가 잠금을 얻지 못해 끝나지 않는다: 옛 서버를 먼저 내린다(`Recreate`).

스키마는 기준선 하나(`crates/store/schema/<종류>/0001_baseline.sql`)에서 시작한다. 기준선 전의 서버들이 이주 1~79로 만든 데이터베이스는, 기준선 전의 마지막 서버가 그 끝(79와 그 뒤의 데이터 단계)까지 올려 둔 것이면 `_sqlx_migrations`의 기록만 기준선의 한 줄로 바뀐다: 표와 행은 그대로이고, 서버 잠금 없이 한 트랜잭션으로(PostgreSQL은 그 표를 잠그고) 바꾸므로 그 데이터베이스를 쓰는 서버가 돌고 있어도 된다. 그 끝까지 오지 않은 것(밀린 이주, 다른 체크섬, 실패한 이주, 남은 데이터 단계)은 무엇이 다른지 말하며 거절한다: 기준선 전의 마지막 서버로 먼저 연다. 기록이 바뀐 데이터베이스는 기준선 전의 서버가 열지 않으므로, 그 서버로 되돌리는 길은 바꾸기 전의 백업을 되살리는 것뿐이다.

PostgreSQL은 17, 18, 19를 지원한다: 웹앱만 고친 것이 아니면 변경마다 CI가 시험을 도는 판이다(18에서는 시험 전부를, 17과 19에서는 스토어의 통합 시험 `crates/store/tests/store`를). `docker/compose.yaml`은 `postgres:18`로 뜬다. 주판(17, 18, 19처럼 주 버전의 수)마다 데이터 디렉터리의 형식이 달라, 이미지의 주판을 올리면 새 컨테이너는 옛 판이 만든 볼륨에서 시작을 거절한다(데이터는 그대로 남는다). 주판을 올릴 때는 `docker/backup.sh`로 뜬 백업을 새 볼륨에 `docker/restore.sh`로 되살리거나 `pg_upgrade`를 거친다.

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `database_kind` | `FVOCI_DATABASE_KIND` | 문자열 | `"sqlite"` | `sqlite` 또는 `postgres` |
| `database_connection` | `FVOCI_DATABASE_CONNECTION` | 문자열 | `"./fvoci.db"` | 데이터베이스에 닿는 곳. SQLite는 파일의 경로이고(`:memory:`는 서버 하나만의 메모리 DB), 다른 쓰기를 기다리는 시간을 `?busy_timeout_ms=5000`처럼 붙일 수 있다(기본 5초). 파일은 WAL로 연다(읽기와 쓰기가 서로 기다리지 않는다. 커밋마다 디스크에 닿는다: `synchronous=FULL`). 서버가 떠 있는 동안 곁에 `<DB 파일>-wal`과 `-shm`이 있고, 마지막 커밋은 `-wal`에 있을 수 있다: 폴더는 이 기기의 디스크여야 하고(네트워크 파일 시스템 불가), 떠 있는 서버의 DB는 파일 복사가 아니라 `sqlite3 <DB 파일> ".backup <사본>"`이나 `VACUUM INTO`로 백업한다(내린 서버의 것은 파일을 복사하되, 곁에 `-wal`이 남아 있으면 닫히지 못하고 내려간 것이니 함께 복사한다). 롤백 저널로 쓰던 파일은 처음 열 때 WAL로 바뀐다. PostgreSQL은 `postgres://사용자:비밀번호@호스트:포트/DB?sslmode=disable` 꼴의 URL이고, 붙일 수 있는 것은 `sslmode`, `sslrootcert`, `sslcert`, `sslkey`, `application_name`, `search_path`, `connect_timeout`(초)이다. 그 밖의 꼴이나 붙인 것이면 시작하지 않는다. 다른 테이블과 한 DB를 나눠 쓰려면 `search_path`로 이 서버만의 스키마를 준다. 비밀번호가 들어 있을 수 있어 로그에 남기지 않는다 |
| `database_max_connections` | `FVOCI_DATABASE_MAX_CONNECTIONS` | 정수(1 이상) | `20` | 서버가 여는 데이터베이스 연결 풀의 최대 연결 수. 0이면 시작하지 않는다 |
| `database_connect_attempts` | `FVOCI_DATABASE_CONNECT_ATTEMPTS` | 정수(0 이상) | `5` | 시작할 때 DB에 닿으려고 해 보는 횟수. 0이어도 한 번은 해 본다 |
| `server_lock_wait_seconds` | `FVOCI_SERVER_LOCK_WAIT_SECONDS` | 정수(0 이상) | `30` | 시작할 때 다른 프로세스가 쥔 서버 잠금(위)을 기다리는 시간(초). 지나면 시작을 거절한다. 0이면 기다리지 않는다. 재시작에서 앞선 서버가 내려가는 시간보다 길게 둔다 |
| `org_creation` | `FVOCI_ORG_CREATION` | 문자열 | `"first_user_only"` | 서버가 조직을 하나만 두는가(`first_user_only`), 여럿을 받는가(`open`). `open`은 `database_kind`가 `sqlite`이면 시작하지 않는다(SQLite는 조직 하나만 담는다). PostgreSQL에서는 조직의 행을 행 단위 보안 정책이 갈라 둔다. 서버가 붙는 DB 역할이 슈퍼유저이거나 `BYPASSRLS`이면 그 정책이 걸리지 않는다: `open`이면 DB에 아무것도 만들기 전에 시작을 거부하고, `first_user_only`면 경고를 남기고 시작한다. 그 밖의 값은 시작 오류다. 가입(`register`)이 이 값을 따른다: `first_user_only`에서는 첫 사용자만 가입해 조직을 시작하고 그 뒤의 사람은 초대 링크로만 들어온다(403 `invitation_required`). `open`에서는 가입하는 사람마다 새 조직을 시작한다 |

`docker/compose.yaml`로 띄운 설치의 백업과 갱신은 그 곁의 스크립트가 한다(쓰는 법은 그 머리 주석). 세 스크립트는 compose가 그 파일들과 `.env`를 거절하거나 경고하면(값의 `$`는 compose에게 변수이고, 서비스는 그것을 뺀 값을 받는다: 받는 값을 지키려면 `$`와 그 뒤 이름을 지우고, `$`를 주려면 `$$`로 쓰거나 값을 작은따옴표로 감싼다. 그렇게 바뀐 비밀 키나 비밀번호는 그것으로 봉한 것을 열지 못한다) 아무것도 바꾸지 않고 멈추고, compose의 말을 그것이 따온 값 없이 보인다(아는 말은 줄 번호와 그 말로, 나머지는 줄 수로). `docker/backup.sh`는 DB를 `pg_dump -Fc`로, 파일이 볼륨에 있으면 그 파일을, 그리고 `.env`와 compose 파일의 사본을 주인만 읽는 폴더(700, 파일은 600) 하나에 뜨고, `--keep-days`(기본 14)보다 오래된 그 프로젝트의 백업을 지운다. 파일이 S3에 있으면 DB만 뜨고 그렇게 말한다(버킷은 운영자가 맡는다). `docker/restore.sh`는 빈 DB와 빈 파일 볼륨에만 되살린다: fvoci의 컨테이너가 있거나, 표가 하나라도 있거나, 파일이 있거나, 백업의 `made`가 다른 프로젝트(`--other-project`로 허용)·파일 저장 방식을 가리키거나, `FVOCI_SECRET_KEY`가 없거나(`--key-not-checked`로 허용) 백업과 다르면 아무것도 바꾸지 않고 거절한다. 되살린 뒤 `up`이 스키마를 올리고, 관리 소켓의 `search/reindex`로 검색 색인을 다시 만들고 `files/reconcile`로 기록과 파일을 대조한다. `docker/update.sh`는 새 compose 파일과 `.env`를 먼저 검사해 모자란 줄을 이름하고 아무것도 멈추지 않은 채 끝낸다. 지나면 `pull`, 새 이미지의 `--migrate --check`(돌던 fvoci는 그대로, 열 수 없는 데이터베이스면 아무것도 바꾸지 않고 멈춘다), 백업, fvoci 멈춤, `up`(마이그레이션), 준비와 ping의 커밋 확인 순서이고, 내려가 있던 시간과 스키마의 판을 찍고, 그 전에 돌던 이미지에 `fvoci-server:before-update-<프로젝트>-<UTC 시각>` 태그를 붙여 둔다. 내리는 마이그레이션은 없다: 스키마가 바뀐 뒤 되돌리는 길은 백업에서 되살리는 것뿐이다.

### PostgreSQL 컨테이너의 튜닝

`docker/compose.yaml`의 `postgres`는 PostgreSQL 18 이미지의 기본값과 Docker의 기본 `/dev/shm`(64 MiB)으로 뜬다: `shared_buffers=128MB`, `work_mem=4MB`, `wal_compression=off`, `jit=on`. 써 보기에는 되지만 실서버에서는 정렬과 해시(목록의 정렬, 창 함수, 해시 조인)가 `work_mem`을 넘어 디스크의 임시 파일로 넘치고, `org_wall`의 행 단위 보안이 걸린 짧은 질의도 큰 표에서 비용 추정이 `jit_above_cost`(기본 100000)를 넘으면 실행보다 길 수 있는 LLVM 컴파일을 먼저 한다. 큰 값(`blocks.body`·`body_versions.body`의 JSONB, `body_docs.state`·`body_updates.bytes`의 Yjs 상태와 갱신)은 TOAST가 느린 `pglz`로 압축하고, 체크포인트 뒤 처음 고치는 페이지는 그 사본(최대 8 KiB)이 압축 없이 WAL에 들어간다. 그래서 실서버에서는 기기에 맞춘 값을 저장소에 넣지 않는 자기 파일(아래는 `override.yaml`)에 적어 `docker/compose.yaml` 뒤에 준다: `docker compose -f docker/compose.yaml -f override.yaml up -d`. 뒤의 파일이 앞의 것을 덮고(`command`는 통째로 바뀐다) `.env`는 첫 파일 곁의 것을 읽으므로 `docker/compose.yaml`을 먼저 둔다. 세 스크립트에도 같은 `-f` 둘을 준다(`docker/backup.sh -f docker/compose.yaml -f override.yaml`, `restore.sh`와 `update.sh`도): `restore.sh`와 `update.sh`는 받은 파일들로 `up`하고 `backup.sh`는 그 사본을 백업에 넣으므로, 빠뜨리면 PostgreSQL이 기본값으로 다시 만들어지고 백업에 그 파일이 없다. 값을 바꾸면 `up`이 postgres 컨테이너를 새로 만들어 그동안 DB가 내려간다: fvoci를 먼저 멈추고(`stop fvoci`) `up -d`한다.

4 vCPU, 24 GiB, SSD(NVMe나 클라우드의 블록 SSD)에서 fvoci와 OpenSearch가 함께 도는 서버 하나의 `override.yaml`:

```yaml
services:
  postgres:
    shm_size: "2g"
    command: ["postgres",
      "-c", "max_connections=40",
      "-c", "shared_buffers=4GB",
      "-c", "effective_cache_size=16GB",
      "-c", "work_mem=32MB",
      "-c", "maintenance_work_mem=512MB",
      "-c", "wal_compression=zstd",
      "-c", "default_toast_compression=lz4",
      "-c", "checkpoint_timeout=15min",
      "-c", "max_wal_size=4GB",
      "-c", "min_wal_size=512MB",
      "-c", "random_page_cost=1.1",
      "-c", "effective_io_concurrency=200",
      "-c", "maintenance_io_concurrency=200",
      "-c", "max_parallel_workers=4",
      "-c", "jit=off"]
```

- `shm_size: "2g"`: 병렬 질의의 일꾼들이 나눠 쓰는 메모리(`dynamic_shared_memory_type=posix`)가 `/dev/shm`에 잡히고, 모자라면 그 질의가 `could not resize shared memory segment … No space left on device`로 실패한다. 병렬 해시 하나가 `work_mem`×`hash_mem_multiplier`(2)×(`max_parallel_workers_per_gather`+1)까지, 여기서는 192MB를 쓰므로 `work_mem`을 올리면 함께 올린다(2g는 그런 해시 열 개쯤). 한도일 뿐 미리 잡지 않고, `shared_buffers`는 여기 들지 않는다(`shared_memory_type=mmap`).
- `max_connections=40`(기본 100): fvoci의 풀(`database_max_connections`, 기본 20)과 서버 잠금의 전용 연결 하나에, 마이그레이션(`migrate`, `update.sh`의 `--migrate --check`)과 `pg_dump`, `psql`로 하는 점검의 몫, 슈퍼유저 몫(`superuser_reserved_connections`, 3)을 더한 수다. 연결마다 정렬·해시마다 `work_mem`을 쓸 수 있으므로 쓰지 않을 연결을 열어 두지 않는다. `database_max_connections`를 올리면 그만큼 올린다.
- `shared_buffers=4GB`, `effective_cache_size=16GB`: `shared_buffers`는 메모리의 약 17%로, DB만 도는 기기의 흔한 값(25%)보다 낮다: fvoci와 OpenSearch(그 힙과 그것이 기대는 OS의 페이지 캐시)가 같은 메모리를 쓴다. `effective_cache_size`는 메모리를 잡지 않는 계획기의 짐작으로, `shared_buffers`와 OS의 페이지 캐시를 합쳐 DB가 기대할 수 있는 크기다: 클수록 색인 스캔을 고른다. `wal_buffers`는 기본(-1)이 `shared_buffers`의 1/32을 16MB까지 잡으므로 여기서는 이미 16MB다.
- `work_mem=32MB`, `maintenance_work_mem=512MB`: `work_mem`은 질의의 정렬·해시 하나의 한도다(병렬 일꾼은 저마다, 해시는 그 2배까지). 40 연결이 하나씩 쓰면 1.25GB다. `maintenance_work_mem`은 색인을 만들 때(마이그레이션, `restore.sh`의 `pg_restore`)와 `VACUUM`의 것이고, autovacuum 일꾼(`autovacuum_max_workers`, 3)도 저마다 이만큼까지 쓴다. 넘치는지는 `pg_stat_database`의 `temp_files`·`temp_bytes`로 본다.
- `wal_compression=zstd`, `default_toast_compression=lz4`: 체크포인트 뒤 처음 고치는 페이지마다 WAL에 들어가는 페이지 전체(full-page write)를 zstd로 줄인다: 본문과 Yjs 상태처럼 큰 행을 자주 고치는 쓰기의 WAL과 디스크 쓰기가 준다. `default_toast_compression`은 TOAST가 압축하는 큰 값(행이 약 2KB를 넘게 하는 JSONB 본문, Yjs의 bytea)을 `pglz`보다 빠르게 압축하고 푼다. 그 뒤로 쓰는 값에만 걸리고, 있던 값은 다시 쓰일 때까지 `pglz`다. 이미지의 PostgreSQL은 lz4와 zstd를 넣어 지었다.
- `checkpoint_timeout=15min`, `max_wal_size=4GB`, `min_wal_size=512MB`: 체크포인트가 드물수록 full-page write가 준다. 대신 죽은 뒤 되살릴 때 최대 15분 치 WAL을 다시 읽고, 볼륨의 `pg_wal`이 `max_wal_size` 안팎까지 찬다. `pg_stat_checkpointer`의 `num_requested`가 `num_timed`보다 빨리 늘면 `max_wal_size`가 모자란 것이다. `checkpoint_completion_target`은 기본이 이미 0.9다.
- `random_page_cost=1.1`, `effective_io_concurrency=200`, `maintenance_io_concurrency=200`(기본 4, 16, 16. 17은 4, 1, 10): SSD에서는 임의 읽기가 순차 읽기와 거의 같은 값이므로 색인 스캔을 고르게 하고, 한 번에 미리 읽는 요청을 늘린다. 하드 디스크면 기본으로 둔다.
- `max_parallel_workers=4`(기본 8): 병렬 일꾼을 코어 수까지로 막아 fvoci와 OpenSearch의 몫을 남긴다. `max_parallel_workers_per_gather`와 `max_parallel_maintenance_workers`는 기본 2가 이 기기에 맞다. `max_worker_processes`는 기본 8로 둔다: 논리 복제 런처가 그 자리 하나를 차지하므로 4로 내리면 병렬 일꾼은 셋까지만 뜬다.
- `jit=off`: fvoci의 질의는 짧아서 JIT로 얻을 것이 적고, JIT가 걸리는 것(비용 추정만 큰 것)은 컴파일이 실행보다 길어지기 쉽다.

다른 기기에서는 비율을 따른다. 메모리를 따르는 것은 `shared_buffers` 15~25%(fvoci와 OpenSearch가 같은 기기면 낮은 쪽), `effective_cache_size` 65~70%, `maintenance_work_mem` 2% 안팎, `work_mem`은 그 `max_connections`배가 5% 안팎, `shm_size`는 `work_mem`의 60배 안팎이다: 8 GiB면 `1536MB`·`5GB`·`128MB`·`8MB`·`"512m"`, 16 GiB면 `3GB`·`11GB`·`256MB`·`16MB`·`"1g"`, 64 GiB면 `12GB`·`44GB`·`1GB`·`64MB`·`"4g"`. 코어를 따르는 것은 `max_parallel_workers`(코어 수), `max_parallel_workers_per_gather`와 `max_parallel_maintenance_workers`(그 절반, 4까지), `max_worker_processes`(`max_parallel_workers`+1 이상: 8코어면 `9`)이고, 병렬 해시 하나의 몫이 일꾼 수를 따라 커지므로 `shm_size`도 함께 늘린다. `max_connections`는 메모리가 아니라 `database_max_connections`를 따르고, WAL과 체크포인트의 값은 쓰는 양과 디스크를 따른다.

이 절이 말하는 기본값은 `postgres:18`의 것이다. `postgres:19`는 `jit=off`와 `default_toast_compression=lz4`가 이미 이미지의 기본값이고, `postgres:17`은 데이터 체크섬이 꺼진 채(`data_checksums=off`) 만들어지며 `effective_io_concurrency=1`, `maintenance_io_concurrency=10`이 기본값이다. 위 `override.yaml`의 값 15개는 17, 18, 19 어디서나 그대로 걸린다(19에서 두 줄은 기본값과 같을 뿐이다).

새 설치를 `postgres:17`로 띄우거나 백업을 17에 되살릴 때는 `override.yaml`의 `postgres`에 `image: postgres:17`과 함께 `environment: { PGDATA: /var/lib/postgresql/17/docker }`를 준다. 17 이미지는 `VOLUME /var/lib/postgresql/data`를 선언하므로, `docker/compose.yaml`의 마운트(`postgres-data:/var/lib/postgresql`)만 두면 데이터가 이름 붙은 볼륨이 아니라 그 안에 따로 걸리는 익명 볼륨에 들어간다: `down`이나 `rm`으로 컨테이너를 지운 뒤 다시 `up`하거나 볼륨을 다른 기기로 옮기면 빈 클러스터가 뜨고 표가 없다(지운 컨테이너의 데이터는 `down -v`가 아니었으면 떨어져 나간 익명 볼륨에 남는다). 값을 바꾼 뒤의 `up`처럼 컨테이너를 지우지 않고 새로 만들 때는 compose가 익명 볼륨을 새 컨테이너에 이어 붙여 데이터가 남는다. `PGDATA`를 주면 데이터가 18, 19와 같은 자리(볼륨의 `<주판>/docker`)에 들어가 새 컨테이너에서도 남는다. 마운트를 `/var/lib/postgresql/data`로 바꾸는 길은 쓰지 않는다: compose는 뒤 파일의 `volumes`를 앞 파일의 것과 합치므로 기본 마운트도 남고, 같은 마운트로 18이나 19에 올리면 그 이미지가 시작을 거절한다. `roles`는 `postgres:18` 그대로 둔다(18의 `psql`이 17 서버에 역할을 만든다).

## 키: 파일 저장소

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `file_storage` | `FVOCI_FILE_STORAGE` | 문자열 | `"local"` | `local`(디스크) 또는 `s3`(S3와 S3 호환 저장소). 그 밖의 값이면 시작하지 않는다 |
| `file_storage_path` | `FVOCI_FILE_STORAGE_PATH` | 문자열 | `"./files"` | `local`일 때 파일을 두는 디렉터리. 파일은 그 아래 `.tmp/writing`에 다 쓴 뒤 제자리로 옮긴다(같은 파일시스템이라 원자적이다. `tmp_path`와 상관없다). 이 DB의 서버만 쓴다: 관리 소켓의 파일 대조(`removeUnrecorded`)는 서버가 만드는 모양의 자리(`<조직>/<보드>/<비밀>`)에서 이 DB가 이름하지 않는 파일을 지운다. 준비 상태(`GET /api/v1/ready`)는 5초에 한 번까지 `<file_storage_path>/.fvoci-ready`를 같은 길로 써서 읽고 지워 보고, 디렉터리가 없으면 만든다 |
| `max_file_bytes` | `FVOCI_MAX_FILE_BYTES` | 정수(1 이상, 바이트) | `104857600` | 올릴 수 있는 파일의 가장 큰 크기(100 MiB). 0이나 음수는 시작 오류다(예전에는 0이 "한도 없음"이었다). `upload_part_bytes` × 10,000(S3 멀티파트의 부분 수 한도)을 넘어도 시작 오류다. 요청 하나로 올리는 파일(`uploadFile`)은 그에 더해 `upload_part_bytes`까지다. 사용자 한 명이 열린 업로드에 걸어 둘 수 있는 크기는 이 한도와 `max_import_bytes` 가운데 큰 것의 두 배다 |
| `max_org_storage_bytes` | `FVOCI_MAX_ORG_STORAGE_BYTES` | 정수(`max_file_bytes` 이상, 바이트) | `53687091200` | 조직 하나가 저장할 수 있는 파일과 보관된 옛 판 바이트의 누적 상한(50 GiB, 휴지통 포함). 넘게 올리거나 가져오면 413(`storage_full`, `limit`)으로 거절한다. `max_file_bytes`보다 작으면 시작 오류다. 조직의 한도로 읽는다(`App::limits`) |
| `file_storage_s3.access_key_id` | — | 문자열 | `""` | 액세스 키. **비워 두면 S3 클라이언트가 환경에서 자격을 읽는다**(`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` 등. 아래 "S3의 예") |
| `file_storage_s3.secret_access_key` | — | 문자열 | `""` | 비밀 키. `access_key_id`가 있을 때만 쓴다. 로그에 남기지 않는다 |
| `file_storage_s3.bucket` | `FVOCI_FILE_STORAGE_S3_BUCKET` | 문자열 | `""` | 버킷. `s3`이면 비어 있을 수 없다. 자격에는 읽고 쓰고 지우는 것 말고 버킷을 나열하는 권한(`s3:ListBucket`)도 있어야 한다: 없는 파일을 물으면 그것 없이는 403이 오고, 준비 상태(`GET /api/v1/ready`)는 작은 파일(`<key_prefix>/.fvoci-ready`)을 써서 읽고 지워 본다(5초에 한 번까지). 그래서 자격은 그 키에도 쓰고 읽고 지울 수 있어야 한다. 버전 관리가 켜진 버킷에서는 검사마다 그 키에 옛 판과 지움 표시가 쌓이므로, 그 키에 옛 판을 하루 뒤 지우는 수명 주기 규칙(NoncurrentVersionExpiration, ExpiredObjectDeleteMarker)을 건다. 또한 버전 관리를 켰다면 접두어 전체에도 `NoncurrentVersionExpiration`과 `ExpiredObjectDeleteMarker`를 함께 건다: 그러지 않으면 보드·조직·계정이 영구 삭제된 뒤에도 객체의 옛 판 바이트가 버킷에 남는다 |
| `file_storage_s3.key_prefix` | `FVOCI_FILE_STORAGE_S3_KEY_PREFIX` | 문자열 | `""` | 모든 객체 키 앞에 붙는 접두어. 앞뒤의 `/`는 떼고 쓴다. 버킷의 이 접두어 아래는 이 DB의 서버만 쓴다(`file_storage_path`의 파일 대조와 같다) |
| `file_storage_s3.region` | `FVOCI_FILE_STORAGE_S3_REGION` | 문자열 | `""` | 버킷의 리전. 비우면 S3 클라이언트의 기본값이다 |
| `file_storage_s3.endpoint` | `FVOCI_FILE_STORAGE_S3_ENDPOINT` | 문자열 | `""` | AWS가 아닌 S3 호환 저장소의 호스트(포트 포함, 스킴 없이). 있으면 경로 방식(`호스트/버킷/키`)으로 부른다. 비우면 AWS다 |
| `file_storage_s3.https` | `FVOCI_FILE_STORAGE_S3_HTTPS` | 참·거짓 | `false` | `endpoint`에 HTTPS로 닿는지. 거짓이면 HTTP도 허락한다 |
| `file_storage_s3.server_side_encryption` | `FVOCI_FILE_STORAGE_S3_SERVER_SIDE_ENCRYPTION` | 참·거짓 | `false` | 참이면 저장하는 객체에 서버 쪽 암호화(SSE-S3, `AES256`)를 청한다 |
| `file_storage_s3.timeout_ms` | `FVOCI_FILE_STORAGE_S3_TIMEOUT_MS` | 정수(밀리초) | `0` | S3 요청의 시간 제한. 0 이하면 클라이언트의 기본값이다 |

**S3의 예.** MinIO 같은 호환 저장소에 HTTP로 닿는다.

```json
{
  "file_storage": "s3",
  "file_storage_s3": {
    "bucket": "fvoci",
    "endpoint": "minio.internal:9000",
    "region": "us-east-1",
    "https": false,
    "key_prefix": "files"
  }
}
```

자격은 파일에 적지 않고 환경에서 준다.

```sh
AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=… fvoci-server --config config.json
```

`file_storage_s3`의 키 가운데 자격(`access_key_id`, `secret_access_key`)이 아닌 것은 `FVOCI_FILE_STORAGE_S3_`에 키를 대문자로 이은 환경 변수로도 바꾼다(표의 "환경 변수" 칸). 컨테이너 하나를 여러 환경에 띄울 때 쓴다. 자격은 `FVOCI_*`로 받지 않는다. `access_key_id`를 비우고 위처럼 표준 `AWS_*` 변수로 준다.

청크로 올리는 파일(`createUpload`)은 S3에서 멀티파트 업로드가 된다. 세션을 열 때 최종 자리에 멀티파트 업로드를 시작하고, 부분마다 확인한 뒤 버킷으로 보내고, 완료할 때 객체로 만든다. 버리거나 끝난 세션은 서버가 중단(abort)하고, 그 자리에 파일의 기록이 없는 객체가 있으면(완료의 답을 잃었거나 기록하기 전에 서버가 멈춘 것) 그것도 지운다. 서버가 멈춰 세션이 남으면 다음 정리에서 그렇게 한다. 서버가 그 사이에 멈추면 중단하지 못한 멀티파트 업로드가 버킷에 남아 저장 요금이 나가므로(S3의 멀티파트 업로드에는 만료가 없다), 버킷에 수명 주기 규칙 `AbortIncompleteMultipartUpload`를 둔다. 세션은 24시간까지이므로 2일이면 넉넉하다. AWS CLI로는:

```sh
aws s3api put-bucket-lifecycle-configuration --bucket 버킷 --lifecycle-configuration \
  '{"Rules":[{"ID":"abort-incomplete-uploads","Status":"Enabled","Filter":{},"AbortIncompleteMultipartUpload":{"DaysAfterInitiation":2}}]}'
```

Cloudflare R2는 같은 규칙이 기본으로 7일이다. 다른 S3 호환 저장소는 그 문서를 본다.

## 키: 세션

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `session_idle_days` | `FVOCI_SESSION_IDLE_DAYS` | 정수(1 이상) | `14` | 쓰지 않은 채 이만큼의 날이 지나면 세션이 끝난다 |
| `session_max_days` | `FVOCI_SESSION_MAX_DAYS` | 정수(1 이상) | `90` | 로그인한 뒤 이만큼의 날이 지나면 얼마나 쓰든 세션이 끝난다 |

세션 쿠키에 따로 설정은 없다. 이름과 `Secure`는 `server_url`이 정한다(위).

## 키: 로그인 제한과 비밀번호 작업

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `login_pair_free_failures` | `FVOCI_LOGIN_PAIR_FREE_FAILURES` | 정수(0 이상) | `5` | 한 주소가 한 계정에 연달아 실패해도 기다리지 않는 횟수 |
| `login_pair_base_delay_seconds` | `FVOCI_LOGIN_PAIR_BASE_DELAY_SECONDS` | 정수(1 이상, 10년 이하) | `30` | 공짜가 아닌 첫 실패 뒤의 기다림(초). 실패마다 두 배가 된다 |
| `login_pair_max_delay_seconds` | `FVOCI_LOGIN_PAIR_MAX_DELAY_SECONDS` | 정수(1 이상, 10년 이하) | `3600` | 한 주소가 한 계정에서 기다리는 가장 긴 시간(초). `login_pair_base_delay_seconds`보다 작을 수 없다 |
| `login_account_max_failures` | `FVOCI_LOGIN_ACCOUNT_MAX_FAILURES` | 정수(1 이상) | `100` | 어디서든 연달아 이만큼 실패하면 계정을 잠근다 |
| `login_account_lock_seconds` | `FVOCI_LOGIN_ACCOUNT_LOCK_SECONDS` | 정수(1 이상, 10년 이하) | `3600` | 계정이 잠겨 있는 시간(초) |
| `login_address_attempts_per_minute` | `FVOCI_LOGIN_ADDRESS_ATTEMPTS_PER_MINUTE` | 정수(0 이상) | `20` | 한 주소가 1분에 해 볼 수 있는 로그인과 가입, 초대 링크의 미리보기와 수락. 한 주소의 시도는 한 번에 하나씩 지나간다. 0이면 주소로 세지 않는다(모든 사용자가 한 주소에서 오고 프록시도 없는 서버) |
| `password_hash_concurrency` | `FVOCI_PASSWORD_HASH_CONCURRENCY` | 정수(1 이상) | `4` | 한꺼번에 하는 비밀번호 해시와 확인. 하나에 19 MiB와 몇 밀리초의 처리기가 든다 |
| `password_hash_queue` | `FVOCI_PASSWORD_HASH_QUEUE` | 정수(0 이상) | `16` | 해시할 차례를 기다릴 수 있는 요청의 수 |
| `password_hash_queue_seconds` | `FVOCI_PASSWORD_HASH_QUEUE_SECONDS` | 정수(1 이상) | `5` | 차례를 기다리는 시간(초). 넘으면 다시 해 보라고 답한다(429) |

## 키: 프록시

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `trusted_proxies` | `FVOCI_TRUSTED_PROXIES` | 문자열 목록 | `[]` | 서버 앞의 역방향 프록시의 주소나 주소 범위(`10.0.0.0/8` 꼴). 이 주소에서 온 요청만 `X-Forwarded-For`가 말하는 주소에서 온 것으로 본다. 비우면 클라이언트가 서버에 바로 닿는다고 본다. 터널(cloudflared 같은 것)도 이런 프록시다: 서버가 보는 터널의 주소를 적는다(`docker/compose.yaml`의 프로필 `tunnel`이면 그 머리 주석). 읽는 머리글은 `X-Forwarded-For`뿐이고 `CF-Connecting-IP`는 읽지 않는다. 읽을 수 없는 값이 있으면 시작하지 않는다. `server_url`이 https이고 `serve_https`가 거짓인데 이 목록이 비면 시작할 때 경고한다(모든 클라이언트가 프록시 하나로 보여 로그인 제한이 모두에게 한꺼번에 걸리고, 공개 링크로 한 보드를 읽는 연결도 `public_link_connections_per_address`개까지만 받는다) |

`docker/compose.yaml`의 프로필 `tunnel`로 터널을 쓰면 이 목록에 네트워크 `edge`의 범위를 적는다. 그 범위에는 Docker의 포트 프록시가 나르는 연결(호스트 자신에서, Docker Desktop이나 rootless에서 게시한 포트로 들어오는 것)이 오는 곳인 그 네트워크의 게이트웨이도 든다. 다른 연결이 제 주소를 지키는지는 Docker의 설정에 달렸다. 그래서 프로필을 켠 채 `FVOCI_BIND`를 루프백이 아닌 주소(`0.0.0.0` 같은 것)로 두면, 호스트의 어느 망에서든 그 포트에 닿는 사람이 서버가 믿는 `X-Forwarded-For`를 써서 가입·로그인·재설정의 요청 제한을 자기가 고른 주소로 받는다. 기본값(`127.0.0.1`)을 그대로 두거나, 그 포트에는 `X-Forwarded-For`를 덧붙이는 역방향 프록시만 닿게 한다.

## 키: 한도와 휴지통, 임시 파일

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `max_request_bytes` | `FVOCI_MAX_REQUEST_BYTES` | 정수(1 이상, 바이트) | `12582912` | JSON을 받는 API 요청 본문의 가장 큰 크기(12 MiB: 카드의 본문 8 MiB와 가장 큰 속성 약 3.3 MB가 한 요청에 들어간다). 넘으면 413. 본문의 한도(8 MiB)보다 낮추면 그보다 큰 본문은 본문의 한도가 아니라 이 한도에 걸린다. 파일 올리기(`uploadFile`과 `putUploadPart`)에는 쓰지 않는다. 아카이브 가져오기(`importArchive`)는 업로드를 이름으로만 받는 JSON이라 이것을 따른다. 세션 없는 요청(64 KiB)과 WebSocket 메시지(64 KiB. Binary 프레임은 `collab_update_bytes`에 32바이트를 더한 것이 더 크면 그것까지)의 한도는 설정이 아니다 |
| `trash_retention_days` | `FVOCI_TRASH_RETENTION_DAYS` | 정수(1 이상) | `30` | 지운 보드와 블록이 휴지통에 남아 되살릴 수 있는 날. 지나면 영구히 지운다. 어느 블록도(휴지통의 것까지) 가리키지 않는 파일도 그렇게 된 지 이만큼 지나면 지운다 |
| `upload_part_bytes` | `FVOCI_UPLOAD_PART_BYTES` | 정수(5242880–5368709120, 바이트) | `8388608` | 청크로 올릴 때(`createUpload`) 마지막을 뺀 부분 하나의 크기(8 MiB). S3 멀티파트가 받는 부분이 5 MiB에서 5 GiB라서 그 밖이면 시작하지 않는다. 서버 앞의 프록시가 받는 요청 본문보다 작아야 한다(Cloudflare의 Free와 Pro는 100 MB). `file_storage`가 `s3`이어도 부분은 `tmp_path`의 파일로 받아 확인한다. 버킷에 보내는 동안만 메모리에 있고, 그 메모리는 이 크기의 8배까지다(기본 64 MiB). 넘으면 다음 부분은 디스크에서 기다린다 |
| `max_pending_upload_bytes` | `FVOCI_MAX_PENDING_UPLOAD_BYTES` | 정수(`max_file_bytes`와 `max_import_bytes` 이상, 바이트) | `2147483648` | 서버의 열린 업로드(`createUpload`, `createImportUpload`) 모두가 걸어 둘 수 있는 크기의 합(2 GiB). 넘는 업로드를 열면 429이고 `reason`이 `server_uploads_pending`이다. `max_file_bytes`보다 작으면 가장 큰 파일을 언제나 429로 거절하게 되므로 시작 오류다 |
| `max_import_bytes` | `FVOCI_MAX_IMPORT_BYTES` | 정수(1 이상, 바이트) | `268435456` | 가져올 수 있는 아카이브의 가장 큰 크기(256 MiB). 아카이브도 청크로 올리므로 `upload_part_bytes` × 10,000을 넘으면 시작 오류이고, `max_pending_upload_bytes`보다 커도 시작 오류다. 풀린 합계는 1 GiB까지다(고정). 메모리 과할당을 막은 기기(`vm.overcommit_memory=2`)에서는 항목 수를 꾸민 아카이브 하나가 서버를 멈출 수 있다: 읽는 크레이트가 선언된 항목 수만큼 먼저 잡는 자리는 여기서 막지 않는다 |
| `max_concurrent_exports` | `FVOCI_MAX_CONCURRENT_EXPORTS` | 정수(1 이상) | `2` | 서버가 한꺼번에 하는 내보내기(`exportContainerArchive`, `exportWorkspaceArchive`)의 수. 한 사용자는 하나씩이다(설정이 아니다). 내보내기는 아카이브를 다 보낼 때까지(클라이언트가 끊거나 `export_stall_seconds` 동안 아무것도 받지 않을 때까지) 자리를 잡는다. 넘으면 429이고 `reason`이 `server_exports_busy`(사용자의 것이 돌고 있으면 `export_running`)다. 0은 시작 오류다 |
| `export_stall_seconds` | `FVOCI_EXPORT_STALL_SECONDS` | 정수(1 이상) | `60` | 내보내기의 내려받기가 이 시간(초) 동안 한 바이트도 보내지 못하면 그만둔다: 임시 파일을 지우고 자리를 돌려주고 연결을 끊는다. 보낸 양은 서버의 버퍼(커널의 송신 버퍼와 HTTP 계층, 합쳐 몇 MiB)를 떠난 양이라, 클라이언트는 이 시간에 그만큼은 받아야 끊기지 않는다. 0은 시작 오류다 |
| `max_concurrent_imports` | `FVOCI_MAX_CONCURRENT_IMPORTS` | 정수(1 이상) | `2` | 서버가 한꺼번에 하는 가져오기(`importArchive`)의 수. 한 사용자는 하나씩이다. 넘으면 429이고 `reason`이 `server_imports_busy`(사용자의 것이 돌고 있으면 `import_running`)이며 업로드는 열린 채 남는다. 열린 업로드의 한도(`max_pending_upload_bytes`)는 따로다. 0은 시작 오류다 |
| `max_comment_versions` | `FVOCI_MAX_COMMENT_VERSIONS` | 정수(1 이상) | `50` | 댓글마다 남기는 고치기 전 글(판)의 수. 맨 처음 쓴 글은 늘 남고 그 수에 든다. 넘으면 그 다음으로 오래된 판부터 버린다(`getCardCommentVersions`). 줄이면 그 뒤에 고쳐지는 댓글부터 줄어든다. 0은 시작 오류다 |
| `max_file_versions` | `FVOCI_MAX_FILE_VERSIONS` | 정수(1 이상) | `50` | 파일마다 남기는 판의 수. 지금의 판도 그 수에 든다. 새 판을 올리거나 지난 판으로 되돌려 넘으면 가장 오래된 판부터 버리고, 그 판의 바이트는 정리 작업이 지운다(`getFileVersions`). 줄이면 그 뒤에 새 판이 쓰이는 파일부터 줄어든다. 0은 시작 오류다 |
| `wiki_page_depth` | `FVOCI_WIKI_PAGE_DEPTH` | 정수(1 이상) | `10` | 위키의 페이지가 들어갈 수 있는 깊이(맨 위의 페이지가 1). 더 깊게 만들거나 옮기거나 되살리거나 복제하거나(페이지, 위키 통째) 아카이브에서 가져오면 400이고 `reason`이 `page_too_deep`이다(만들기와 옮기기는 `errors[0].pointer`가 `/parentId`). 조직의 한도로 읽는다(`App::limits`). 0은 시작 오류다 |
| `wiki_pages` | `FVOCI_WIKI_PAGES` | 정수(1 이상) | `5000` | 위키 하나가 가질 수 있는 페이지의 수(휴지통의 것은 세지 않는다). 넘게 만들거나 되살리거나 복제하거나(페이지, 위키 통째) 아카이브에서 가져오면 400이고 `reason`이 `too_many_pages`, `limit`이 이 값이다. 조직의 한도로 읽는다(`App::limits`). 0은 시작 오류다 |
| `tmp_path` | `FVOCI_TMP_PATH` | 문자열 | `""` | 서버의 임시 파일을 두는 디렉터리. 청크로 올리는 부분은 `file_storage`가 `s3`이어도 여기에 받는다(S3로 보낸 뒤 지운다). 업로드에 필요한 자리는 열린 업로드가 걸어 둔 크기의 합까지다(`max_pending_upload_bytes`). 가져오는 중인 아카이브는 업로드가 끝난 것으로 세어 이 합에 들지 않고, 가져오는 동안 아카이브 하나와 부분 하나, 풀어 낸 파일 항목 하나(`max_file_bytes`까지)의 자리를 차지한다. 동시에 가져오는 수는 `max_concurrent_imports`까지다. 내보내는 아카이브는 쓰고 보내는 동안 자리를 차지하고, 그 크기에는 한도가 없고 수는 `max_concurrent_exports`까지다. 끝나지 않은 업로드의 부분(`uploads/<uploadId>/`), 가져오는 중인 아카이브(`uploads/<uploadId>.importing-…/`)와 내보내는 중인 아카이브(`exports/`. 다 보내거나 그만두면 지운다)를 둔다. 그 아래 `uploads`와 `exports`는 서버의 것이다: 서버가 시작할 때 어느 업로드의 것도 아닌 것과 내보내다 남은 것을 지운다. 한 디렉터리를 두 서버가 함께 쓰지 않는다(`file_storage_path`도 그렇다. 시작할 때 `file_storage_path/.tmp/writing`을 비운다). 이 청소는 서버 잠금을 쥔 뒤에 하므로 같은 DB의 둘째 서버는 하지 못하지만, DB가 다른 두 서버가 한 디렉터리를 쓰는 것은 막지 않는다. 비우면 `file_storage_path` 아래 `.tmp`다. 시스템의 임시 디렉터리는 메모리에 있을 수 있어 쓰지 않는다 |

## 키: 본문의 공동 편집

카드 본문을 함께 고치는 동안 그 Y 문서는 서버가 자기 실행 파일을 작업자 모드로 띄운 자식 프로세스(작업자)가 쥔다. 서버와 작업자는 표준 입출력의 프레임으로만 이야기하고, 작업자는 파일·소켓·DB를 열지 않는다. 작업자가 죽거나 시간 안에 답하지 않으면 서버가 죽이고 다시 띄우며, 문서는 저장된 마지막 상태에서 다시 열린다. 잇달아 죽으면 다시 띄우기 전에 0.1초부터 두 배씩, 10초까지 기다린다. 한 문서에 대한 요청이 15분 안에 두 번 작업자를 죽이면 그 문서는 10초 동안 닫히고(`document_unavailable`), 거듭될 때마다 두 배씩, 5분까지 닫힌다.

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `collab_workers` | `FVOCI_COLLAB_WORKERS` | 정수(1–64) | `1` | 띄우는 작업자의 수. 문서는 열 때 가장 적게 받은 작업자에게 간다. 한 조직의 한 사용자가 보낸 업데이트는 작업자가 몇이든 한 번에 하나씩 보낸다(두 카드에 함께 친 것은 차례로 받는다): 작업자를 끝낸 업데이트의 보낸 사람은 다음 것을 보내기 전에 거절된다 |
| `collab_documents_per_worker` | `FVOCI_COLLAB_DOCUMENTS_PER_WORKER` | 정수(1 이상) | `500` | 작업자 하나가 한꺼번에 쥐는 문서의 수. 그 밖에 열린 문서가 없는 카드의 기록을 카드에 쓰려고 잠깐 여는 문서는 서버 전체로 넷까지 따로 쥔다(모자라면 기다린다). 작업자는 문서들의 상태와 아직 접지 않은 되돌리기용 업데이트를 합쳐 (`collab_worker_memory_bytes` − 512 MiB) ÷ 32까지만 쥔다(기본값으로 32 MiB). 512 MiB는 업데이트 하나를 제자리에서 받아 고치는 동안 쓰는 여유 몫이고(증분 캐시로 100,000노드·8 MiB 본문 전 형상에서 라이브 편집·삭제 복구 시 추가 힙 스파이크 +0.0 MB, 최악 상주 힙은 `marks and formulas` 210 MB를 쟀다. 모자라 죽은 작업자는 다시 띄우고, 그때 받던 업데이트를 보낸 사람은 잠시 거절된다), 32는 상태 1바이트가 메모리에서 차지하는 양이다(노드가 많은 문서에서 프로그램이 청하는 것만 20–26배를 쟀고, 할당기가 그 둘레에 두는 것을 더했다). 넘게 하는 열기와 업데이트는 거절한다 |
| `collab_state_bytes` | `FVOCI_COLLAB_STATE_BYTES` | 정수(16777216–1073741824, 바이트) | `16777216` | 문서 하나의 저장된 상태의 가장 큰 크기(16 MiB). 8 MiB·100,000노드 본문의 문서(꾸밈이 가장 촘촘한 본문 포함)와 편집·삭제 여유를 담는다. 문서는 지운 것도 쥐고 있어 고칠수록 자란다. 넘게 하는 업데이트는 거절하고, 4분의 3에 이르면(새로 만든 직후에 이미 가까운 문서는 한도의 4분의 1 이상 더 자란 뒤) 서버가 카드의 본문에서 문서를 새로 만든다(편집자는 새로 받는다). 낮춰도 이미 저장된 문서는 받아들인 그대로 열리고, 넘는 문서는 본문을 카드에 쓴 뒤 새로 만든다 |
| `collab_update_bytes` | `FVOCI_COLLAB_UPDATE_BYTES` | 정수(1–`collab_state_bytes`, 바이트) | `65536` | 클라이언트가 보내는 업데이트 하나의 가장 큰 크기(64 KiB). 업데이트 하나에 드는 작업자의 시간은 주로 문서의 크기를 따른다. 변경마다 문서의 긴 목록을 걷게 하거나 긴 글을 여러 군데 자르게 지은 업데이트는 걸음이 3 Mi(약 315만)를 넘으면 거절한다(설정이 아니다. 지나가는 변경 하나, 또는 자르는 변경의 글자 256개가 한 걸음이다). 그런 변경은 문서를 상태에서 읽을 때마다 다시 걷는다: 받아들이면 문서를 읽는 걸음이 1 Mi(약 105만)를 넘게 되는 업데이트는(설정이 아니다) 고칠 것이 있으면 제자리에서 고친 뒤 기록하고 본문을 카드에 쓴 다음 곧바로 본문에서 문서를 다시 만들어(`docReset`) 보낸 업데이트를 잃지 않는다(기록한 뒤 카드에 쓰기 전에 서버가 내려가도 다시 열 때 기록에서 되살려 다시 만든다). 걷게 지은 변경이 없어도 세션 수천 개가 앞 세션에 이어 쓴 문서는 그 걸음에 이른다(세션마다 클라이언트 하나이고, 라이브러리는 앞 세션을 기다리는 변경들을 훑는다): 걸음이 거의 없다가도 가장 먼저 읽히는 세션 하나의 첫 문단으로 한 번에 넘는다. 그런 업데이트가 오기 전에 서버는 세션 하나가 더 먼저 읽히면 문서의 지금 변경들에 들 수 있는 걸음(센 걸음에서, 기다리는 변경을 훑는 몫만 가장 많은 경우인 문서의 변경 수 × 클라이언트 수 ÷ 128로 바꾼 것)을 보고 문서를 본문에서 미리 다시 만든다: 마지막 연결이 방을 나가 문서를 닫을 때는 그것이 한도의 8분의 1에 이르렀거나 클라이언트가 2,048개이면(쥔 연결이 없어 아무도 다시 받지 않는다), 방이 비지 않는 문서는 4분의 3에 이르면 연결들이 쥔 채로(`docReset`). 업데이트가 더하는 변경과 남의 변경 안에 쓰거나 지워 둘로 자르는 변경은 그 셈에 없다(하나마다 클라이언트 128개에 한 걸음): 클라이언트 수천 개의 문서에 그런 변경 수만 개를 넣는 업데이트도(고칠 것이 함께 있어도) 제자리에서 고쳐 받아들이고 곧바로 본문에서 다시 만든다. 걸음을 세기 전에 저장된 문서가 그 한도를 넘으면 읽지 않고 카드의 본문에서 다시 만든다(카드에 아직 쓰이지 않은 업데이트는 버린다). 한 클라이언트의 지우는 범위가 거꾸로 가거나 겹치는 업데이트도 거절한다. 문서가 이름 짓는 클라이언트가 16,384개를 넘게 하거나(상태를 읽는 시간이 클라이언트 수의 제곱으로 는다), 한 클라이언트의 변경이 지운 것까지 524,288개를 넘게 하거나(그 클라이언트의 변경을 자를 때마다 뒤의 변경을 모두 옮긴다), 문서 전체의 변경이 지운 것까지 786,432개(`768 × 1024`)를 넘게 하는 업데이트도 거절하고, 그 4분의 3에 이른 문서는 본문에서 다시 만든다(문서 전체 변경은 새로 만든 뒤 한도의 4분의 1 이상 더 자란 경우). 이것들보다 오래 걸리게 지은 업데이트는 `collab_worker_timeout_ms`에 작업자와 함께 끝나고, 저장되지 않으며, 보낸 연결은 닫히고, 보낸 사용자의 업데이트는 10초부터 두 배씩 1시간까지 거절된다(같은 조직에서 하루 안에 그런 사용자가 세 번째인 때부터는 조직 전체가). 받아들일 때의 한도다: 이미 기록된 업데이트는 낮춘 뒤에도 다시 적용된다 |
| `collab_worker_timeout_ms` | `FVOCI_COLLAB_WORKER_TIMEOUT_MS` | 정수(1 이상, 밀리초) | `10000` | 작업자의 답을 기다리는 시간. 넘으면 그 요청은 거절되고 작업자는 죽였다가 다시 띄운다 |
| `collab_recheck_seconds` | `FVOCI_COLLAB_RECHECK_SECONDS` | 정수(1–3600, 초) | `30` | 본문을 함께 고치는 방(`contracts/websocket.md` 6절)에 든 연결마다 아직 들어 있어도 되는지, 쓰는지 읽기만 하는지를 다시 묻는 간격. 보드에서 빠지거나 역할이 내려간 사람은 보통 이 안에, 길어야 이것의 두 배(묻는 데 걸리는 시간이 한 간격까지다) 뒤에 내보내지거나 읽기 전용이 된다. 한 번 물을 때 방에 든 사용자 하나에 DB 읽기 다섯 번이고(같은 사용자가 같은 방식(비밀번호, 어느 공급자)으로 만든 세션의 연결 여럿은 한 번에 묻는다), 한 방에서 여덟 사용자씩 함께 묻는다. 저장소가 답하지 못했거나 한 간격 안에 답을 얻지 못한 사용자는 2초 뒤에 한 번 더 묻고, 그래도 답이 없으면 그 연결을 방에서 내보낸다. 조직이 비밀번호를 받지 않게 되면(`ssoRequired`) 그 공급자로 만들지 않은 세션의 연결은 이 간격을 기다리지 않고 곧바로 그 조직의 방에서 나간다 |
| `collab_documents_per_connection` | `FVOCI_COLLAB_DOCUMENTS_PER_CONNECTION` | 정수(1–1024) | `8` | WebSocket 연결 하나가 한꺼번에 들 수 있는 방의 수. 넘는 `joinDoc`은 `docRefused`이고 `reason`이 `too_many_documents_joined`다 |
| `collab_connections_per_document` | `FVOCI_COLLAB_CONNECTIONS_PER_DOCUMENT` | 정수(1–10000) | `50` | 본문 하나의 방에 들 수 있는 연결의 수(같은 사용자의 탭도 하나씩 센다). 넘는 `joinDoc`은 `docRefused`이고 `reason`이 `document_full`이다. 변경 하나는 방의 다른 연결 모두에게 간다 |
| `collab_connections_per_user` | `FVOCI_COLLAB_CONNECTIONS_PER_USER` | 정수(1–10000) | `5` | 본문 하나의 방에 든 연결 가운데 한 사용자의 것일 수 있는 수(탭과 기기마다 하나). 넘는 `joinDoc`은 `docRefused`이고 `reason`이 `too_many_connections_joined`다. 한 사용자가 방(`collab_connections_per_document`)을 혼자 채우지 못하게 한다 |
| `collab_frames_per_second` | `FVOCI_COLLAB_FRAMES_PER_SECOND` | 정수(10–1000000) | `50` | 연결 하나가 1초에 보낼 수 있는 방의 Binary 프레임(sync 메시지와 커서)의 수. 쉬고 난 뒤에는 이만큼을 한꺼번에 보낼 수 있고 그 뒤로 1초에 이만큼씩이다. 문서 전체로 답하는 `joinDoc`과 sync step 1은 다섯으로, `leaveDoc`은 하나로 센다. 서버가 그 연결의 일을 하느라 읽지 못한 동안 쌓인 것은 그 시간만큼 더 받는다(60초 치까지): 한도 안에서 보낸 클라이언트가 서버가 느려서 넘지는 않는다. 넘는 `joinDoc`은 `docRefused`, 넘는 프레임은 그 프레임의 방에서 내보내기(`docLeft`)이고 `reason`이 `too_many_frames`다. 커서 프레임만은 넘어도 버릴 뿐이다 |
| `collab_queue_messages` | `FVOCI_COLLAB_QUEUE_MESSAGES` | 정수(16–65536) | `512` | 연결 하나에 보내려고 기다리게 두는 방의 메시지의 수. 바이트로는 `collab_state_bytes`의 두 배까지다(설정이 아니다). 넘으면 그 연결을 그 방에서 내보내고(`docLeft`, `connection_too_slow`) 방의 다른 연결은 그대로 받는다. 구독의 메시지(512개, 넘으면 연결을 닫는다)와는 따로 센다 |
| `collab_body_versions` | `FVOCI_COLLAB_BODY_VERSIONS` | 정수(2–1000) | `50` | 카드 하나의 본문에서 남겨 두는 판의 수(본문을 통째로 쓰면 앞의 본문과 쓴 본문이 함께 남으므로 둘부터다). 새 판이 생겨 이 수를 넘으면 가장 오래된 판부터 지운다(판의 번호는 이어서 센다. 편집자가 손으로 남긴 판도 함께 센다). 판 하나는 본문 전체이고, 판이 보여 주는 이미지의 파일은 그 판이 남아 있는 동안 지우지 않는다. 낮추면 카드마다 다음 판이 생길 때 넘는 판이 지워진다 |
| `body_versions_max_bytes` | `FVOCI_BODY_VERSIONS_MAX_BYTES` | 정수(1 이상, 바이트) | `67108864` | 카드 하나의 남겨 둔 본문 판들이 합쳐 차지할 수 있는 비압축 JSON 바이트 예산(기본 64 MiB). 새 판이 남아 합계가 이 값을 넘으면 가장 오래된 판부터 지운다(가장 새 판 하나는 이 값보다 커도 남긴다). 지워진 판만 보여 주던 파일은 정리 대상이 된다. 낮추면 카드마다 다음 판이 생길 때 넘는 판이 지워진다. 0은 시작 오류다 |
| `collab_version_minutes` | `FVOCI_COLLAB_VERSION_MINUTES` | 정수(1–1440, 분) | `10` | 본문을 함께 고치는 동안 판을 남기는 간격. 어느 판에도 없는 첫 변경에서 이만큼 지난 뒤 본문이 카드에 쓰일 때(또는 그다음 변경을 받을 때) 판을 남긴다. 그 밖에 가장 새 판 뒤로 본문을 고친 사람이 모두 방을 떠날 때(고치지 않은 사람은 남아 있어도 된다), 본문을 통째로 쓸 때, 서버가 멈출 때 남기고, 죽은 서버가 남기지 못한 판은 다음 시작 때 남긴다. 편집자가 지금 판을 남기라고 하면(`keepCardBodyVersion`, `keepPageBodyVersion`) 그때 남기고(손으로 남긴 판 뒤 1분 안에는 남기지 않는다), 간격은 그 뒤의 첫 변경부터 다시 센다. 본문이 가장 새 판과 같으면 남기지 않는다. 고치다가 멈춘 채 방에 머무는 동안에는 시간이 지나도 남기지 않고, 떠나거나 다시 고칠 때 남긴다 |
| `collab_worker_memory_bytes` | `FVOCI_COLLAB_WORKER_MEMORY_BYTES` | 정수(1073741824 이상이고 536870912 + `collab_state_bytes`의 32배 이상, 바이트) | `1610612736` | 작업자의 주소 공간 한도(1.5 GiB, 유닉스의 `RLIMIT_AS`). `collab_state_bytes`를 올리면 이것도 함께 올려야 한다. 모자라면 서버가 시작하지 않고 필요한 값을 알린다. 유닉스에서는 그 밖에 코어 파일 0, 열린 파일 16, 자기 프로세스 그룹, 리눅스에서는 서버가 죽으면 함께 죽기가 걸린다. Windows에는 이 한도가 없다 |

## 키: 공유와 보이는 것

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `allow_public_links` | `FVOCI_ALLOW_PUBLIC_LINKS` | 참·거짓 | `false` | 참이면 보드와 위키를 읽기 토큰으로 공개 공유할 수 있다. 거짓이면 어느 읽기 토큰도 맞지 않는다 |
| `feature_flags` | — | 문자열에서 문자열로의 객체 | `{}` | 웹앱에 넘기는 기능 표시(`getClientConfig`의 `featureFlags`) |
| `name_display` | `FVOCI_NAME_DISPLAY` | 문자열 | `"username"` | 웹앱이 다른 사용자의 이름을 보이는 방식(`getClientConfig`의 `nameDisplay`) |
| `show_email_addresses` | `FVOCI_SHOW_EMAIL_ADDRESSES` | 참·거짓 | `false` | 사용자 목록에 이메일을 싣는지 |

## 키: 관리자 소켓과 웹훅

관리 소켓에서 `POST /api/v1/admin/users/{username}/password-reset`을 보내면 `url`에 재설정 링크를 받는다. 운영자가 사용자에게 전달한다. 링크는 256비트 비밀값을 URL 조각(`#` 뒤)에 담고 서버에는 그 해시만 남는다. 비밀번호가 있는, 쓰이는 계정과 삭제를 기다리는 계정만 발급받는다. `POST /api/v1/password-reset/complete`에 `token`과 새 `password`를 보내면 1시간 안에 한 번 쓸 수 있고, 비밀번호 변경과 모든 세션 삭제가 함께 저장된다. 열린 WebSocket도 닫힌다. 관리 경로는 HTTP 리스너에서 열리지 않는다. 메일이 닿지 않는 사람의 가입은 `POST /api/v1/admin/users/{username}/email-verification`이 링크처럼 확인한다(로그인시키지는 않는다; 확인을 기다리는 가입이 없으면 404). 구성원 링크로 한 가입이면 들어간 조직에 운영자의 기록(`email_verified_by_operator`)이 남고, 제 조직을 세우는 가입과 손님은 그 전에 속한 조직이 없어 기록이 없다.

계정 삭제: 사용자가 계정 설정에서 삭제를 요청하면(비밀번호, 또는 비밀번호가 없는 계정은 방금 한 SSO 로그인으로 본인임을 보인다) 계정은 30일 동안 기다린다. 그동안 로그인하면 삭제 취소만 할 수 있고, 그 계정이 만든 초대 링크는 아무도 들이지 않는다. 기다림이 끝난 계정은 세션 정리 작업(시작할 때와 한 시간마다)이 지운다. 운영자는 관리 소켓에서 `POST /api/v1/admin/users/{username}/erase`로, 요청하지 않은 계정도 기다리지 않고 지울 수 있다(204, 그런 이름의 계정이 없으면 404, 아래의 유일한 관리자는 409). 유예 중에 비밀번호를 잊은 사람도 재설정 메일(또는 운영자의 재설정 링크, 비밀번호 지정 명령)로 새 비밀번호를 정하고, 로그인하면 취소 화면이 나온다. 재설정은 취소가 아니다. 지우기가 시작된 계정과 지운 계정에는 재설정 링크가 나가지 않고, 나간 링크도 듣지 않는다. 지우면 계정은 모든 조직·워크스페이스·보드·팀에서 빠지고 이름·이메일·비밀번호·SSO 연결·세션·설정·사이드바 분류·구독, 아무도 쓰지 않은 그 계정의 초대 링크가 지워지며, 식별자만 남은 행이 남는다. 쓴 것(카드, 페이지, 댓글, 버전)은 남고 작성자는 "탈퇴한 사용자"로 보인다. 감사 기록은 남고 그 사람의 신원(연결한 SSO 주소나 subject)만 지워진다. 혼자 관리하던 보드는 그 워크스페이스의 다른 관리자에게 넘어간다. 넘길 관리자가 없는 보드와, 혼자 관리하던 워크스페이스는 관리자 없이 남고 서버 로그에 경고로 남는다(조직 관리자가 들어가 정한다). 서버의 유일한 관리자와, 다른 사람이 있는 조직의 유일한 관리자는 지워지지 않는다(409, 로그에 남고 다음 정리 때 다시 본다). 아무도 없이 혼자 쓰던 조직은 계정과 함께, 안에 든 것까지 지워진다(파일의 객체는 파일 정리 작업이 이어서 지운다). 지우기를 시작할 때 그 조직의 SSO 설정을 지우고 초대 링크를 무효로 해 아무도 들어오지 못하게 한다. 그래도 그 사이에 누가 들어왔으면 조직은 남기되 관리자, SSO 설정, 초대 링크가 없는 상태가 되고 로그에 경고가 남는다(운영자가 관리자를 정해 주어야 한다). 지우는 것은 그 계정 자신의 주소로 온 초대뿐이고, SSO 공급자가 알려 준 주소로 온 초대는 건드리지 않는다. 서버의 첫 조직(서버 관리자가 따라오는 조직)은 예외로, 혼자 쓰고 있어도 계정과 함께 지워지지 않고 그 유일한 관리자도 지워지지 않는다. 이미 내려받은 아카이브, 이미 보낸 메일, 서버 로그, DB 백업에 남은 것은 지우지 못한다.

메일 주소 바꾸기: 사용자가 계정 설정에서 새 주소와 지금 비밀번호를 내면(틀린 비밀번호는 로그인과 같은 한도에 걸린다; 비밀번호가 없는 계정은 먼저 정한다) 메일을 보내는 서버는 새 주소로 확인 링크(`/confirm-email#토큰`, 24시간, 한 번, 서버에는 해시만)를 보내고, 링크를 열 때까지 계정의 주소는 그대로다(재설정 메일도 옛 주소로 간다). 링크는 로그인하지 않은 브라우저에서 열어도 된다. 바뀌면 옛 주소로 나가 있던 재설정 링크는 듣지 않고, 옛 주소로 알림 메일이 한 통 간다(새 주소는 적지 않는다). 기다리는 바꾸기는 계정마다 하나이고 7일 뒤 사라지며, 다시 요청하면 앞의 것을 무르고, 취소할 수 있고, 비밀번호가 바뀌거나 계정 삭제를 요청하면 함께 끝난다. 새 주소가 다른 계정이나 확인을 기다리는 가입의 것이면 답은 같고(204) 링크만 나가지 않는다. 기다리는 바꾸기는 주소를 쥐지 않으므로, 그 사이 다른 사람이 그 주소로 가입하면 링크는 듣지 않는다. 링크와 알림은 재설정 메일의 줄로 가고 `mail_reset_per_account_hour`를 계정마다(링크와 알림을 따로), 받는 주소마다 센다. 계정의 수는 링크를 청할 때마다 센다: 메일이 나가지 않은 요청도 하나이므로, 뒤에 받는 메일의 수로 그 주소가 남의 것인지 가를 수 없다. 메일을 보내지 않는 서버는 주소를 확인하지 않는 가입과 같이 비밀번호만으로 바로 바꾸고, 그때는 남의 주소를 409(`email_taken`)로 말한다. 삭제를 기다리는 계정은 바꾸지 못한다.


| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `serve_admin_socket` | `FVOCI_SERVE_ADMIN_SOCKET` | 참·거짓 | `false` | 참이면 관리 API를 유닉스 소켓으로 연다(비밀번호 재설정, 관리자 지정 등) |
| `admin_socket_path` | `FVOCI_ADMIN_SOCKET_PATH` | 문자열 | `"/var/tmp/fvoci_local.socket"` | 그 소켓의 경로 |
| `block_change_webhooks` | `FVOCI_BLOCK_CHANGE_WEBHOOKS` | 문자열 목록 | `[]` | 바뀐 블록마다 그 블록을 `POST`로 받는 주소들 |

## 키: 지표

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `metrics_address` | `FVOCI_METRICS_ADDRESS` | 문자열(`호스트:포트`) | `""` | Prometheus 지표를 내는 주소. 비어 있으면 지표를 모으지 않고 아무것도 듣지 않는다. 주소가 있으면 API와 **다른 리스너**를 그 주소에 열고 `GET /metrics`만 Prometheus의 글 형식으로 답한다(그 밖의 길은 404이고, 거절은 API와 같은 `application/problem+json`이며 그것이 가리키는 `/problems`의 글도 여기서 내준다). API의 리스너에는 지표의 길이 없다. 호스트는 이름(글자·숫자·`-`의 마디를 점으로 이은 것, 끝 마디는 글자로 시작), 점 넷의 IPv4, 괄호 안의 IPv6(`[::1]:9092`)이고 빠질 수 없다(`0x7f.1`, `127.1` 같은 다른 IPv4 표기는 받지 않는다). 모든 인터페이스는 `0.0.0.0:9092`(IPv4)나 `[::]:9092`(IPv6)이고, `listen_loopback_only`가 참이면 모든 인터페이스에서 듣게 되는 주소(이 둘, `[::ffff:0.0.0.0]`, 그리로 풀리는 이름)는 시작 오류다. 그 밖의 주소는 적힌 대로 듣는다. 포트는 0–65535의 수(앞에 0 없이)이고, 0은 빈 포트 아무것이며 고른 주소를 로그에 남긴다. 꼴이 틀리면(`:9092`, `localhost`, 괄호 없는 IPv6, 영역이 붙은 IPv6, 서비스 이름) 무엇이 틀렸는지 말하며, 그 주소에서 들을 수 없으면(이미 쓰임, 이 기기의 주소가 아님) 시작하지 않는다. 인증은 `metrics_token`을 따르고, `serve_https`와 관계없이 평문 HTTP다. 모니터링만 닿는 안쪽 주소(루프백, 사설망)에 묶는다. 요청 로그에 남기지 않는다. 긁을 때 DB를 세는 조회가 둘 있다: 열린 업로드는 5초에 한 번까지, 살아 있는 세션은 1분에 한 번까지(세션 표 전체를 훑는다). 긁기가 아무리 잦아도 DB에 묻는 것은 그만큼이다. 둘은 나란히 돌고 긁기는 각각 2초까지 기다리며, 같은 조회는 DB가 답할 때까지 동시에 하나다. 준비 상태(`GET /api/v1/ready`의 확인, 5초 동안 기억한다)도 긁을 때 보고 `fvoci_ready`로 낸다 |
| `metrics_token` | `FVOCI_METRICS_TOKEN` | 문자열 | `""` | 지표 리스너의 `GET /metrics`가 요구하는 bearer 토큰. 비어 있으면 인증이 없다. 값이 있으면 `Authorization: Bearer <토큰>`이 맞아야 하고, 없거나 틀리면 401(`WWW-Authenticate: Bearer realm="metrics"`, 틀린 것에는 `error="invalid_token"`)이며 그때는 아무것도 읽지 않는다. `Authorization` 머리글이 둘 이상이면 어느 것도 받지 않고 순서와 값에 상관없이 401(`error="invalid_request"`)이다. 환경 변수가 빈 값이면 파일의 값을 둔다. 글자열이 아닌 값(따옴표 없는 수)은 그 값을 되풀이하지 않는 오류로 시작하지 않는다. 설정돼 있으면 32자 이상이어야 하고, 짧으면(환경 변수로 준 것도) 키 이름과 '32자 이상'만 말하고 값도 그 길이도 말하지 않는 오류로 시작하지 않는다. 비교는 SHA-256 다이제스트를 일정 시간에 견준다. 글자는 RFC 6750의 것(영문자, 숫자, `-._~+/`, 끝의 `=`)만 받고, 아니면 시작하지 않는다. 로그, 오류 문구, `Debug` 출력에 나오지 않는다. 토큰이 있어도 평문 HTTP이므로 지표 리스너는 안쪽 주소에 둔다 |

## 키: 메일 보내기

메일은 선택 사항이다. 보내는 길은 `mail_transport`가 정한다: SMTP 릴레이(직접 설치), 또는 공급자의 HTTP API(Cloudflare Email Service, Amazon SES, OCI Email Delivery). 비우면 `smtp_host`가 있을 때 SMTP, 없으면 메일을 보내지 않는다. 고른 길이 읽지 않는 키(다른 공급자의 키, SMTP가 아닌 길의 `smtp_host`·`smtp_user`)가 있으면 그 키를 말하며 시작하지 않고, 고른 길에 필요한 키가 빠져도 그 키를 말하며 시작하지 않는다. 비밀 값(Cloudflare의 API 토큰, SES의 비밀 키, OCI 개인 키 파일의 내용)은 로그·오류·`Debug`에 나오지 않는다. SES에는 `SendEmail`(API v2)로 MIME 그대로(`Content.Raw`) 한 사람씩 보낸다. OCI에는 `SubmitEmail`(`/20220926/actions/submitEmail`)로 제목·평문·HTML을 한 사람씩 보내고, 요청은 API 키로 서명한다(HTTP Signatures, `rsa-sha256`). OCI의 메시지는 2 MB(머리글·base64 포함)까지다. OCI는 승인되지 않은 보내는 사람만이 아니라 없는 구획·정책도 `404 NotAuthorizedOrNotFound`로 답하고, Cloudflare는 틀린 계정 ID에 `404`로 답한다: 둘 다 모든 메일이 같은 답을 받는 설정의 잘못이라 길이 막힌 것으로 본다(아래). SES 샌드박스 계정은 확인된 주소에만, 하루 200통·초당 1통까지 보낸다(`mail_per_second` 기본 1이 맞는다). 공급자 쪽의 도메인·보내는 사람 승인은 운영자의 일이다: SES가 승인되지 않은 보내는 사람을 거절하면(`400`) 그 메일의 영구 실패이고 로그에는 응답 코드만 남는다. 메일이 켜지면 로그인 화면에서 비밀번호 재설정 메일을 요청하고, 사람 관리·손님 초대 화면에서 기존 초대 링크를 여러 주소로 보낸다. 초대 메일은 주소마다 따로 보낸다. 구성원 링크는 그대로 보내고 만료·사용 횟수를 바꾸지 않는다. 손님 링크는 한 사람이 한 번 쓰므로, 받는 사람마다 같은 보드·역할·만료의 한 번 쓰는 손님 링크(자식 링크)를 보낼 차례에 따로 만들어 보낸다. 자식 링크는 요청에 쓴 링크(부모)와 받는 주소를 적고, 만든 사람은 보낸 사람이다. 영구 실패했거나 포기한 메일의 링크는 거둔다(보낸 사람이 다시 보낼 수 있다). 시간 초과도 다시 시도하므로, 공급자가 늦게 받은 메일이 포기되면 받는 사람의 링크는 거둬져 있을 수 있다. 부모 링크는 그대로 남고 틀일 뿐이라, 쓰인 뒤에도 거두거나 만료되기 전에는 메일이 나간다. 부모 링크를 거두면 아직 쓰이지 않은 자식 링크도 함께 거둔다. 사람 관리 화면과 보드 공유 창의 손님 링크 목록은 자식 링크의 받는 주소를 보인다. 보내기 전마다 초대 권한과 링크를 다시 확인한다. 비밀번호 재설정 요청의 답은 계정 유무와 관계없이 같고, 계정 조회와 보내기는 배경 작업이다. 비밀번호 없는 계정에는 재설정 메일을 보내지 않는다. 요청은 출처 주소마다 로그인과 같은 설정(`login_address_attempts_per_minute`)으로 따로 세고, 이메일로는 세지 않는다: 남이 내 주소로 요청해도 나는 막히지 않는다. 한 계정에 가는 메일은 `mail_reset_per_account_hour`까지이고, 넘은 요청도 답은 같으며 아무것도 잠그지 않는다. 재설정을 마치면 그 계정(사용자 이름과 이메일)의 로그인 잠금과 재설정을 마친 주소의 기다림이 끝난다(다른 주소의 기다림은 남는다). 메일은 영어·한국어 평문과 HTML로 보낸다: 받는 사람의 언어 설정에 맞는 글이 있으면 그것, 없으면(`auto`, `ja` 같은 것) 보낸 사람의 설정과 요청한 화면의 언어, 그것도 없으면 영어다. `/ready`는 SMTP에 연결하지 않는다. 메일의 링크는 `server_url`에서 나오므로 받는 사람이 닿는 공개 주소여야 한다(루프백 주소면 시작할 때 경고를 남긴다).

`docker/compose.yaml`은 이 표의 환경 변수를 모두 서버에 넘긴다(`mail_transport`, `email_verification`, 공급자의 키와 한도는 `.env`가 정할 때만 넘겨 덮어 쓴 `config.json`의 값을 지우지 않는다. `smtp_*`·`mail_from`과 비밀의 `*_FILE`은 늘 넘기므로, 비어 있어도 그 파일의 값보다 앞선다). 비밀(`FVOCI_SMTP_PASSWORD`, `FVOCI_CLOUDFLARE_API_TOKEN`, `FVOCI_SES_SECRET_ACCESS_KEY`, OCI 개인 키의 PEM인 `FVOCI_OCI_PRIVATE_KEY`)은 `.env`에서 Docker secret으로 받고, 서버에는 `/run/secrets/` 아래의 파일로 닿는다(OCI의 것은 그 파일이 `oci_private_key_file`이 된다). 쓰지 않는 공급자의 비밀은 `.env`에 빈 값으로 둔다(`docker/.env.example`): 그 줄이 없으면 compose가 시작하지 않는다.

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `email_verification` | `FVOCI_EMAIL_VERIFICATION` | 불 | `false` | 켜면 가입은 계정을 만들지 않고(답은 202, 세션 없음) 가입한 주소로 확인 링크(`/verify-email#토큰`, 24시간, 한 번)를 보낸다. 링크를 열면 그때 계정과 조직·첫 워크스페이스를 만들고 그 브라우저를 로그인시킨다. 확인은 가입한 사람만 끝낸다: 가입한 브라우저(202가 준 `HttpOnly` 쿠키 `fvoci_signup`, https에서는 `__Host-fvoci_signup`, 7일, 세션이 아니다)에서는 링크만으로, 다른 곳에서는 가입 때 정한 비밀번호를 함께 내야 한다(없으면 403 `password_required`, 틀리면 로그인의 401과 같은 한도, 링크는 쓰이지 않는다). 그 전의 가입은 누구의 조회에도 나오지 않고 이름과 주소만 쥐며(같은 이름·주소의 가입은 409. 다만 가입한 브라우저에서 다시 가입하면 앞 가입을 버리고 받아, 주소를 고쳐 같은 사용자 이름으로 바로 가입할 수 있고 초대 링크의 쓰임도 돌아온다), 7일 안에 확인하지 않으면 주기 청소가 지워 이름과 주소가 다시 풀린다. 확인 전의 로그인은 비밀번호가 맞을 때만 403(`email_unverified`)이고, 틀리면 계정이 없을 때와 같은 401이다. 확인 메일은 재설정 메일과 같은 줄·차례로 가고 한 주소에 시간당 `mail_reset_per_account_hour`통까지다. 메일을 보낼 길이 없으면 이 키를 말하며 시작하지 않는다. 켜기 전에 만든 계정, 공급자로 만든 계정은 그대로 들어온다. 조직의 공급자가 주소를 증명하며 들어오면(새 계정이든 있던 계정이든) 같은 주소로 그 조직의 초대 링크에서 기다리던 가입은 그때 지워진다(이름과 링크의 쓰임이 풀린다). `getLoginOptions`의 `emailVerification`이 웹에 알린다 |
| `mail_transport` | `FVOCI_MAIL_TRANSPORT` | 문자열 | `""` | `smtp`, `cloudflare`, `ses`, `oci`. 비우면 `smtp_host`가 있을 때 `smtp`, 없으면 메일을 보내지 않는다. 다른 값은 시작하지 않는다 |
| `mail_api_endpoint` | `FVOCI_MAIL_API_ENDPOINT` | 문자열 | `""` | 공급자의 요청이 공급자의 공식 주소 대신 가는 곳(`https://호스트[:포트]`, 경로·질의·자격 없이). 테스트의 가짜 공급자를 위한 것이고 `http`는 루프백 IP로 쓴 주소(`127.0.0.1`, `[::1]`)만 받는다(이름은 받지 않는다). 공급자로의 요청은 프록시 환경 변수를 쓰지 않는다. 비우면 공급자의 주소(Cloudflare `https://api.cloudflare.com`, SES `https://email.{ses_region}.amazonaws.com`, OCI `https://cell0.submit.email.{oci_region}.oci.oraclecloud.com`: 다른 렐름은 이 키로 준다). SMTP에는 쓰지 않는다 |
| `cloudflare_account_id` | `FVOCI_CLOUDFLARE_ACCOUNT_ID` | 문자열 | `""` | `cloudflare`일 때 필수. Email Service로 보내는 Cloudflare 계정 ID(영문자·숫자) |
| `cloudflare_api_token` | `FVOCI_CLOUDFLARE_API_TOKEN` | 문자열 | `""` | `cloudflare`일 때 필수. 메일 보내기 권한이 있는 API 토큰(`Authorization: Bearer`). 로그에 남기지 않고, 글자열이 아닌 값은 그 값을 되풀이하지 않는 오류로 시작하지 않는다 |
| `ses_region` | `FVOCI_SES_REGION` | 문자열 | `""` | `ses`일 때 필수. SES가 보내는 AWS 지역(`us-east-1` 같은 것, 영소문자·숫자·`-`). 끝점의 호스트와 서명(SigV4, 서비스 `ses`)에 쓴다 |
| `ses_access_key_id` | `FVOCI_SES_ACCESS_KEY_ID` | 문자열 | `""` | `ses`일 때 필수. `ses:SendEmail`을 할 수 있는 IAM 사용자의 장기 접근 키(영문자·숫자). 세션 토큰이 필요한 임시 키(`ASIA`로 시작)는 시작하지 않는다 |
| `ses_secret_access_key` | `FVOCI_SES_SECRET_ACCESS_KEY` | 문자열 | `""` | `ses`일 때 필수. 그 비밀 키. 로그에 남기지 않고, 글자열이 아닌 값은 그 값을 되풀이하지 않는 오류로 시작하지 않는다 |
| `oci_region` | `FVOCI_OCI_REGION` | 문자열 | `""` | `oci`일 때 필수. Email Delivery가 보내는 OCI 지역(`us-ashburn-1` 같은 것, 영소문자·숫자·`-`). 승인된 보내는 사람이 있는 지역이어야 한다 |
| `oci_tenancy` | `FVOCI_OCI_TENANCY` | 문자열 | `""` | `oci`일 때 필수. 테넌시의 OCID(서명의 `keyId` 첫 조각) |
| `oci_user` | `FVOCI_OCI_USER` | 문자열 | `""` | `oci`일 때 필수. API 키를 가진 사용자의 OCID |
| `oci_fingerprint` | `FVOCI_OCI_FINGERPRINT` | 문자열 | `""` | `oci`일 때 필수. 그 API 키의 지문: 콘솔이 보이는 꼴대로 소문자 16진수 두 자리 16쌍을 `:`로 이은 것(`12:34:…`). 다른 꼴은 시작하지 않는다 |
| `oci_private_key_file` | `FVOCI_OCI_PRIVATE_KEY_FILE` | 문자열 | `""` | `oci`일 때 필수. API 키의 개인 키 파일(PEM, PKCS#8 또는 PKCS#1, 암호 없이, RSA 2048–8192비트). 시작할 때 읽고, 읽지 못하거나 쓸 수 없으면 이 키를 말하며(내용은 말하지 않고) 시작하지 않는다 |
| `oci_compartment` | `FVOCI_OCI_COMPARTMENT` | 문자열 | `""` | `oci`일 때 필수. 승인된 보내는 사람(`mail_from`)이 있는 구획의 OCID |
| `smtp_host` | `FVOCI_SMTP_HOST` | 문자열 | `""` | SMTP 호스트 이름이나 IP. `mail_transport`가 비었을 때 이것이 있으면 SMTP로 보낸다. 스킴·포트·공백 없이 쓴다 |
| `smtp_port` | `FVOCI_SMTP_PORT` | 정수(1–65535) | `587` | SMTP 포트 |
| `smtp_tls` | `FVOCI_SMTP_TLS` | 문자열 | `"starttls"` | `starttls`(필수 업그레이드), `tls`(처음부터 TLS), `none`(암호화 없음). TLS는 시스템의 인증서 신뢰를 쓴다 |
| `smtp_user` | `FVOCI_SMTP_USER` | 문자열 | `""` | SMTP 인증 사용자. 비밀번호와 함께 주거나 둘 다 비운다 |
| `smtp_password` | `FVOCI_SMTP_PASSWORD` | 문자열 | `""` | SMTP 인증 비밀번호. 로그에 남기지 않는다 |
| `mail_from` | `FVOCI_MAIL_FROM` | 문자열 | `""` | 모든 메일(어느 길이든)의 보내는 주소 하나. 메일이 켜지면 필수이고 시작할 때 검사한다. 표시 이름과 줄바꿈은 받지 않는다 |
| `mail_per_second` | `FVOCI_MAIL_PER_SECOND` | 정수(1 이상) | `1` | 모든 메일(재설정과 초대, 다시 하는 것까지)이 함께 지키는 초당 보내기 시도 수. 기다리는 재설정 메일이 초대보다 먼저 차례를 얻는다. 기본 1은 새 공급자 계정(샌드박스)의 한도에도 맞는다: 공급자가 허락하는 만큼 올린다 |
| `mail_reset_queue` | `FVOCI_MAIL_RESET_QUEUE` | 정수(1 이상) | `200` | 기다리거나 보내는 중인 비밀번호 재설정 메일의 천장. 차면 새 재설정 메일은 버리고(답은 그대로 204) 경고 로그와 `fvoci_password_reset_mail_dropped_total{reason="queue_full"}`에 센다 |
| `mail_reset_per_account_hour` | `FVOCI_MAIL_RESET_PER_ACCOUNT_HOUR` | 정수(1 이상) | `3` | 한 계정에 지난 한 시간에 줄 세운 재설정 메일의 천장(메모리에서 센다). 넘으면 메일을 보내지 않고(답은 그대로 204) 정보 로그와 `fvoci_password_reset_mail_dropped_total{reason="account_limit"}`에 센다. 계정도 출처도 잠그지 않는다 |
| `mail_invitation_queue` | `FVOCI_MAIL_INVITATION_QUEUE` | 정수(`invitation_mail_recipients`의 두 배 이상) | `200` | 기다리거나 보내는 중인 초대 메일의 천장. 서버에 조직이 하나뿐이면(SQLite, 또는 PostgreSQL에 쓰이는 조직이 하나) 그 조직이 다 쓴다. 조직이 여럿이면 한 조직(같은 조직의 워크스페이스를 모두 합해)은 다른 조직들이 남긴 자리의 절반까지 쥔다(한 요청의 받는 사람 수보다 적지는 않다): 줄에 메일이 있는 조직이 `k`이면 저마다 `k + 1`분의 1쯤에 머문다. 줄이 한 요청의 받는 사람 수(`invitation_mail_recipients`)의 여덟 배 이상이면 바쁜 조직이 셋이어도 넷째가 들어갈 자리가 남는다(기본값 200과 20에서는 넷째와 다섯째까지). 한 요청의 메일이 조직의 몫이나 줄에 다 들어가지 않으면 그 요청은 아무것도 보내지 않고 503(`mail_queue_full`) |
| `invitation_mail_recipients` | `FVOCI_INVITATION_MAIL_RECIPIENTS` | 정수(1 이상) | `20` | 초대 메일 요청 하나의 받는 사람 수(중복 포함)의 천장. 넘으면 400(`too_many_recipients`) |
| `invitation_mail_per_hour` | `FVOCI_INVITATION_MAIL_PER_HOUR` | 정수(`invitation_mail_recipients` 이상) | `100` | 워크스페이스마다 지난 한 시간에 줄 세운 초대 메일의 천장(메모리에서 센다). 이 요청의 메일까지 넘으면 아무것도 보내지 않고 429(`invitation_mail_limit`, `Retry-After`) |
| `invitation_mail_per_org_hour` | `FVOCI_INVITATION_MAIL_PER_ORG_HOUR` | 정수(`invitation_mail_recipients`와 `invitation_mail_per_hour` 이상, 작으면 시작하지 않는다) | `300` | 한 조직(그 워크스페이스 모두)이 지난 한 시간에 줄 세운 초대 메일의 천장(메모리에서 센다). 이 요청의 메일까지 넘으면 아무것도 보내지 않고 429(`invitation_mail_limit`, `Retry-After`). 지금은 모든 조직이 이 값이고, 서버는 조직의 한도(`App::limits`)로 읽는다 |
| `invitation_mail_per_address_hour` | `FVOCI_INVITATION_MAIL_PER_ADDRESS_HOUR` | 정수(1 이상) | `5` | 받는 메일 주소 하나에 서버 전체(모든 조직 합산)에서 지난 한 시간에 보낼 수 있는 초대 메일의 천장(메모리에서 센다). 넘은 주소는 응답 차이 없이 조용히 건너뛰며 손님 초대 링크도 만들지 않는다. 0은 시작 오류다 |

보내기는 동시에 넷까지이며 초대 메일은 그 가운데 셋까지 쓴다(하나는 늘 재설정의 몫). 조직이 여럿인 서버에서 한 조직의 초대 메일은 둘까지만 쓴다(하나는 늘 다른 조직의 몫). 재설정과 초대는 줄을 따로 서고, 시도마다 `mail_per_second`의 차례를 받는다. 한 번의 시도는 10초까지다(HTTP 연결은 그 가운데 5초까지, 리다이렉트는 따르지 않는다). 보내는 길의 답이 "잠시 뒤 다시"(SMTP 4xx, `454` 포함, 답이 없는 시간 초과, 연결이 도중에 끊김, HTTP `408`·`429`·`5xx`와 HTTP의 연결·시간 초과, SES의 `ThrottlingException`·`TooManyRequestsException`·`LimitExceededException`(초당·하루 한도), OCI의 `400 LimitExceeded`(분당·하루 한도))이면 1초·4초 뒤(길이 기다릴 시간을 말하면 그만큼이고, 15분보다 길면 그 메일은 포기한다) 두 번 더 시도하고, "영구 실패"(SMTP 5xx, HTTP의 그 밖의 4xx, Cloudflare가 2xx로 받고 `permanent_bounces`에 둔 받는 사람)이면 다시 하지 않는다. 인사에서 서버를 거절하는 릴레이(`554`)는 받는 사람의 거절과 가를 수 없어 메일마다의 영구 실패로 센다. "길이 막힘"(SMTP `530`·`534`·`535`·`538`처럼 서버의 자격을 받지 않는 답, 연결·TLS를 맺지 못함, `smtp_tls`가 바라는 STARTTLS나 자격에 맞는 인증 방식이 없는 릴레이, HTTP `401`·`403`과 따르지 않는 `3xx`, OCI와 Cloudflare의 `404`, SES의 `SendingPausedException`·`AccountSuspendedException`)은 메일의 잘못이 아니므로 그 메일의 시도로 세지 않는다: 1분에 한 번만 시도하고 그 시도는 기다리는 재설정 메일이 먼저 쓴다. 초대 메일은 멈추고, 기다리는 재설정 메일이 없을 때만 그 1분의 시도에 쓰인다. 막힌 길을 15분 넘게 기다린 재설정 메일은 포기하고, 초대 메일은 길이 열릴 때까지 자리를 쥐고 기다린다. 길이 막히면 오류 로그 한 줄과 `fvoci_mail_path_down`(막힘 1, 아님 0)이 남고, 메일을 받거나 길의 답으로 한 메일만 거절하면 길이 열린 것으로 보고 정보 로그를 남긴다. 다시 하기를 기다리는 동안에는 아무것도 쥐지 않는다. 차례를 기다리는 동안 재설정 메일은 아무것도 쥐지 않고, 초대 메일은 조직의 몫과 초대의 몫 하나를 쥔다. 시간 초과도 다시 시도하므로, 길이 이미 받은 메일이 드물게 두 번 갈 수 있다. "잠시 뒤 다시"가 세 번 잇따르면(공급자의 하루 한도로 본다) 초대 메일은 1분(길이 말한 시간, 15분까지) 기다리고 재설정 메일만 시도한다. 보낸 메일이 하나 나오면 초대도 다시 나간다. 포기한 메일은 경고 로그(종류, 시도, 응답 코드만)와 `fvoci_mail_failed_total{kind,class}`(`kind`: `password_reset`·`invitation`·`email_verification`, `class`: `permanent`·`temporary`)에 센다. 공급자의 응답 글, 메일의 주소·토큰은 로그에 남기지 않는다. 재설정 토큰은 보낼 차례를 얻은 뒤 만든다. 작업은 메모리에 있다: 정상 종료는 받은 요청을 끝내고 메일을 10초 더 기다린 뒤 내려가며, 그때 줄에 자리를 쥔 메일(차례나 다시 하기를 기다리거나 보내는 중인 재설정과 가입 확인, 초대)의 수(`recovery`, `invitations`, 주소 없이)를 경고 로그에 남긴다. 그 메일은 보내지지 않는다(보내는 중이던 것은 공급자가 받았을 수도 있고, 계정을 찾느라 아직 자리를 쥐지 않은 것은 세지 않는다). 초당 `mail_per_second`통이므로 그보다 많이 쌓였거나 다시 하기·막힌 길을 기다리는 메일은 사라진다. 프로세스가 갑자기 멈추면 그 수도 남지 않는다. 메일이 꺼져 있으면 운영자가 위 관리 소켓으로 링크를 발급한다.

## 키: 본문 검색

카드와 위키 페이지의 제목과 본문은 따로 띄우는 검색 서버가 있으면 그것으로, 없으면 DB에서 찾는다. 검색 서버는 Elasticsearch나 OpenSearch(둘 다 같은 요청으로 쓴다)에 한국어 형태소 분석기 nori 플러그인(`analysis-nori`)이 있어야 한다. `search_url`이 비어 있으면 DB에서 찾는다: 질문을 공백으로 나눈 낱말마다 제목이나 본문의 평문(`blocks.body_text`)에 글자 그대로 들어 있어야 하고(영어 글자만 대소문자를 가리지 않는다. "환불"로 "환불규정을"은 찾지만 "배포하다"로 "배포합니다"는 못 찾는다), 제목에 낱말이 다 든 것이 먼저, 그다음 최근에 고친 것이 먼저다. 조각은 본문의 첫 일치 둘레 160자쯤이고 그 안의 모든 일치를 표시한다. API, 결과의 꼴, 권한 재확인, 한도는 검색 서버와 같고, 3초 안에 끝나지 않으면 DB가 그 문장을 멈추고(PostgreSQL은 `statement_timeout`, SQLite는 진행 처리기) 503(`search_unavailable`)이다. 이때 `getClientConfig`의 `searchEnabled`는 `true`이고 보낼 것 표에는 적지 않는다(검색 서버 없이 뜨는 서버는 시작할 때 표와 색인의 기억을 지우고, 지우지 못하면 시작하지 않는다. 검색 서버에 남은 옛 색인은 서버가 지우지 않는다: 운영자가 지운다). 검색 서버가 설정되어 있으면 DB는 그것을 대신하지 않는다. 브라우저는 검색 서버에 닿지 않는다. 서버는 `search_index`라는 색인 하나를 스스로 만든다(한국어는 nori로 읽되 복합어는 통째로도 나눠서도 싣고, 같은 글을 표준 분석기로도 한 번 더 싣는다). 그 이름의 색인이 없으면 만들고(서버가 도는 동안 사라져도 다시 만든다), 이 서버가 더 옛 판의 정의로 만든 것이면 지우고 다시 만든다. fvoci의 표지(매핑의 `_meta`에 둔 `fvoci`)가 없는 색인이나 더 새 서버가 만든 색인이 그 이름에 있으면 건드리지 않고, nori가 없으면 색인을 만들지 못한다: 이 경우들은 까닭을 로그에 남기고 서버는 뜨며, 검색은 모두 503(`search_unavailable`)이다(`searchEnabled`는 그대로 `true`이고 DB가 대신하지 않는다. 고친 뒤 서버를 다시 시작한다). 관리 소켓의 다시 색인도 503이고, 검색 서버 없는 서버에서는 404(`search_not_configured`)다. 검색 서버가 답하지 않는 것은 서버의 시작을 막지 않는다: 답할 때까지 1초에서 60초까지 간격을 늘려 가며 다시 묻는다. 검색 요청은 검색 서버를 3초까지 기다리고, 답이 없거나 거절되거나 일부만 답하면 503(`search_unavailable`)이다. `/ready`와 다른 조작은 검색 서버를 기다리지 않는다. 검색 서버의 답은 그대로 믿지 않는다: 답에 든 카드와 페이지마다 DB에서 지금 그 사람이 읽는 것인지 다시 확인하고, 제목과 그릇(보드나 위키)은 DB의 것을 답한다. 그래서 모자라면 그 3초 안에서 다음 것을 더 물어 채운다: 답의 개수는 걸러진 것이 있었는지를 말하지 않는다(역할을 가진 보드와 위키가 1,000개를 넘는 사람은 워크스페이스 전체에서 찾아 걸러 내며, 가장 잘 맞는 10,000개까지만 본다). 서버가 멈출 때는 검색 서버로 가던 요청을 1초까지만 기다린다. 바뀐 것은 저장된 뒤에 보낸다: 카드나 페이지의 제목·본문이 바뀌거나, 카드·페이지(그 아래 페이지와 함께)·보드·위키가 휴지통으로 가거나 돌아오면 그 트랜잭션이 DB의 보낼 것 표(`search_outbox`)에 무엇이 바뀌었는지만 적고(내용은 싣지 않는다), 서버의 주기 작업이 1초마다 표를 읽어 그때 DB에 저장된 것을 검색 서버에 넣거나 지운다. 보내기 전에 검색 서버의 색인이 DB가 기억한 그 색인인지 먼저 묻는다(없는 색인에 문서를 넣으면 두 검색 서버 모두 분석기 없는 색인을 말없이 만들기 때문이다). 보낸 뒤에는 그사이 다시 적히지 않은 행만 표에서 지운다. 문서는 DB에서 읽은 때(마이크로초, 한 서버 안에서 늘 늘어난다)를 판으로 실어 넣고 지운다(`version_type=external_gte`, 문서의 `sent`에도 둔다): 시간이 다 되어 포기한 요청을 검색 서버가 늦게 처리해도 그 뒤의 넣기나 지우기를 되돌리지 못한다. 검색 서버는 지운 문서의 판을 10분 기억하고(`index.gc_deletes`), 서버는 시작해 색인을 다시 쓸 때 색인이 가진 가장 높은 판 뒤에서 시작하고, 검색 서버가 더 높은 판을 가졌다며 거절하면(지운 문서의 판처럼 문서에 남지 않은 것) 그 판 뒤로 넘어가 곧바로 다시 보낸다(시계가 뒤로 갔어도). 댓글, 속성 값, 템플릿 보드의 카드, 보드 안의 카드 템플릿(`isTemplate` 카드)은 보내지 않는다. 검색 서버가 요청을 받지 않으면 표는 그대로 두고 1초에서 60초까지 간격을 늘려 다시 해 보며, 돌아오면 밀린 것을 보낸다. 검색 서버가 문서 하나를 거절하면 그 행만 5초에서 10분까지 간격을 늘려 다시 보낸다(로그에 오류 이름을 남긴다). 표는 검색 서버가 있는 서버에서만 적는다. DB는 표를 어느 색인(이름, 검색 서버가 준 식별자, 정의의 판)에 맞춰 적어 왔는지 기억한다: 색인이 없거나 그 색인이 아니면(처음 켰을 때, 정의의 판이 바뀌었을 때, 검색 서버 없이 뜬 적이 있을 때, 다른 검색 서버를 가리킬 때, 서버가 도는 동안 색인이 사라졌을 때) 색인을 다시 만들고 모든 조직의 보드와 위키를 표에 넣어 처음부터 보낸다. 관리 소켓의 `POST /api/v1/admin/search/reindex`도 같은 일을 한다(답은 표에 넣은 보드와 위키의 수 `containers`. 보내기가 끝날 때까지 검색은 보낸 만큼만 찾는다). 보내기는 DB 하나에 서버 하나라는 전제(`server_lock_wait_seconds`)에 기댄다. 지표: `fvoci_search_requests_total`, `fvoci_search_changes_total`, `fvoci_search_outbox`. 검색 서버로의 요청은 리다이렉트를 따르지 않고 프록시 환경 변수를 쓰지 않는다. 비밀 값(`search_password`, `search_api_key`)은 로그·오류·`Debug`에 나오지 않는다.

| 키 | 환경 변수 | 형 | 기본값 | 뜻 |
|---|---|---|---|---|
| `search_url` | `FVOCI_SEARCH_URL` | 문자열 | `""` | 검색 서버의 주소(`https://호스트[:포트]`, 경로·질의·자격 없이). `http`는 루프백 IP로 쓴 주소(`127.0.0.1`, `[::1]`)만 받는다(이름은 받지 않는다). 비우면 DB에서 찾고, 그때 아래의 자격 키가 있으면 그 키를 말하며 시작하지 않는다 |
| `search_username` | `FVOCI_SEARCH_USERNAME` | 문자열 | `""` | 검색 서버에 HTTP Basic으로 로그인할 사용자(`:`와 제어 문자는 쓰지 못한다). `search_password`와 함께 준다. 두 검색 서버가 모두 받는다 |
| `search_password` | `FVOCI_SEARCH_PASSWORD` | 문자열 | `""` | 그 사용자의 비밀번호(제어 문자는 쓰지 못한다) |
| `search_api_key` | `FVOCI_SEARCH_API_KEY` | 문자열 | `""` | Elasticsearch의 API 키(인코딩된 꼴). `Authorization: ApiKey …`로 보낸다(공백 없는 ASCII). OpenSearch는 이 방식을 받지 않는다. 사용자·비밀번호와 함께 주면 시작하지 않는다 |
| `search_index` | `FVOCI_SEARCH_INDEX` | 문자열 | `"fvoci"` | 이 서버의 색인 이름. 소문자·숫자·`-`·`_`로 100자까지, 글자나 숫자로 시작한다. 서버 하나(DB 하나)에 색인 하나다: 같은 검색 서버를 여러 fvoci 서버가 쓰면 저마다 다른 이름을 준다. 같은 이름을 주면 서로의 색인을 지우고 다시 만들고, DB를 복제한 서버(스테이징)가 운영 서버와 같은 이름을 쓰면 운영 서버의 색인에 쓴다 |
| `search_ca_file` | `FVOCI_SEARCH_CA_FILE` | 문자열 | `""` | 검색 서버 HTTPS 연결에만 추가로 신뢰할 루트 인증서 PEM 파일 경로(시스템 루트에 더하며 OIDC·웹훅·S3·메일 클라이언트의 TLS 신뢰에는 영향을 주지 않는다). `search_url` 없이 주거나 파일을 읽을 수 없거나 유효한 인증서가 없으면 시작하지 않는다 |
| `search_requests_per_second` | `FVOCI_SEARCH_REQUESTS_PER_SECOND` | 정수 | `5` | 한 사람이 1초에 할 수 있는 검색 수(쉬었다가는 한 번에 그 두 배까지). 검색 서버에서 찾든 DB에서 찾든 같다. 넘으면 429와 `Retry-After`. `0`이면 한도가 없다 |

## 키: 조직의 로그인 공급자

조직마다 OIDC 공급자를 하나 둘 수 있다(`PUT /api/v1/org/{orgId}/oidc`, 그 조직의 관리자). 서버가 공급자에게 보내는 요청(discovery, 토큰, 키)은 `https`의 주소에만 가고, 리다이렉트를 따라가지 않고, 프록시를 쓰지 않으며, 응답은 1 MiB와 10초(연결 5초)까지다. 이름을 풀어 나온 주소가 모두 공인 주소일 때만 붙고, 풀어 본 그 주소로 붙는다. 공급자의 client secret은 `FVOCI_SECRET_KEY`(아래 "그 밖의 환경 변수")로 봉인해 저장한다.

| 키 | 환경 변수 | 타입 | 기본값 | 뜻과 범위 |
|---|---|---|---|---|
| `oidc_allow_private_addresses` | `FVOCI_OIDC_ALLOW_PRIVATE_ADDRESSES` | 참·거짓 | `false` | 참이면 공인 주소가 아닌 곳(루프백, 사설망, 링크 로컬 등)의 공급자에도 붙는다. 같은 망 안의 공급자를 쓰는 직접 설치를 위한 것이다. 거짓이면 그런 주소로 풀리는 공급자는 저장도 로그인도 거절된다. `https`가 아닌 주소는 이것과 관계없이 거절된다 |
| `oidc_extra_ca_file` | `FVOCI_OIDC_EXTRA_CA_FILE` | 문자열 | `""` | OIDC 클라이언트가 시스템 루트에 더해 신뢰할 인증서의 PEM 파일 경로. 빈 값은 추가 없음. 읽을 수 없거나 유효한 인증서가 없으면 시작을 거절한다 |
| `oidc_former_provider_days` | `FVOCI_OIDC_FORMER_PROVIDER_DAYS` | 정수(0 이상) | `30` | 조직이 공급자를 다른 issuer로 옮긴 뒤 옛 공급자를 '연결용'으로 남겨 두는 날. 그동안 옛 공급자로는 로그인되지 않고, 새 공급자로 처음 들어온 사람이 옛 공급자에 한 번 로그인해 자기 계정을 이어 받는 데만 쓰인다. 조직 관리자가 일찍 끝낼 수 있고, 지나면 지워진다. 0이면 남기지 않는다 |

조직 관리자는 조직 화면의 통합 로그인(SSO) 절에서 공급자와 자동 가입을 설정하고 콜백 주소를 복사한다. 강제는 지금 저장된 그대로의 공급자로 로그인한 세션에서만 켠다. issuer, client ID, client secret, 로그인 허용 가운데 하나라도 바꾸면 그 전에 만든 세션은 증명이 되지 못하므로 다시 로그인해야 하고, 바꾸는 저장과 강제를 한 번에 하지 못한다(저장된 secret을 그대로 다시 보내는 것은 바꾸는 것이 아니다). 강제 중에는 공급자를 끄거나 지우지 못한다. 강제 중에 issuer, client ID, client secret을 바꿔 저장하면 바로 바뀌지 않고 대기 설정이 되며, 그동안 모두 지금 설정으로 로그인한다. 관리자가 화면의 ‘새 설정으로 로그인해 보기’로 대기 설정의 로그인을 자기 계정으로 끝까지 마치면 그때 대기 설정이 지금 설정이 되고 그 세션이 새 설정의 증명이 된다. 다른 계정으로 끝나거나 그사이 관리자가 아니게 되면 아무것도 바뀌지 않고, 계정을 만들지도 않는다. 다른 issuer로 옮기는 대기 설정이면 새 공급자의 관리자 신원이 아직 어느 계정에도 연결돼 있지 않은 것이 보통인데, 그 신원은 로그인해 보기를 시작한 세션이 지금 공급자(지금 revision)로 만든 것이고, 브라우저가 그 세션을 그대로 갖고 돌아왔으며, 돌아온 때가 그 세션을 만든 지 5분 안일 때에만 그 관리자 자신의 계정에 연결되며, 연결과 적용은 한 트랜잭션이고 감사 기록에 남는다. 세션만 가진 사람은 지금 공급자의 로그인을 다시 통과하지 못하므로 아무것도 연결하지 못한다. 세션이 오래됐으면 화면이 ‘지금 공급자로 다시 로그인’ 단추를 먼저 내놓고, 로그인하면 같은 화면으로 돌아온다. 이미 관리자 계정에 연결된 신원이면 세션의 나이와 관계없이 적용되고, 같은 issuer의 대기 설정은 신원을 연결하지 않는다. 실패하면 아무것도 바뀌지 않는다. 대기 설정은 버릴 수 있고 secret은 돌려주지 않는다. 강제하지 않는 동안에는 바로 바뀌며, 로그인에 쓰이는 값이 바뀌면 대기 설정은 버려진다. 강제를 끈 뒤에 남은 대기 설정은 로그인해 보기로 적용하거나 버릴 수 있다. 대기 저장, 적용, 버림은 조직의 감사 기록에 남는다. 공급자 장애로 로그인할 수 없으면 로컬 관리 소켓의 POST /api/v1/admin/orgs/{orgId}/sso/release로 강제를 해제한다. 공급자 설정과 삭제, 강제 전환은 조직의 감사 기록에 남으며 관리 소켓의 해제는 계정 없이 ‘서버 운영자’로 남는다. 서버 관리자 변경이 인스턴스 자체 조직의 역할을 바꾸면 그 조직에만 같은 행위자로 남는다. 관리 소켓으로 비밀번호를 정하면(그리고 비밀번호 재설정 링크를 만들면) 그 사람이 구성원인 조직마다 한 줄씩 ‘서버 운영자’가 이 구성원에게 한 일로 남고, 비밀번호나 링크는 남기지 않는다. 전체 파일 정리처럼 전역인 관리 소켓 명령은 조직의 기록에 넣지 않으며, 관리 소켓 명령은 모두 비밀 값 없이 명령·대상 사용자 ID(해당할 때)·결과·운영자 표시를 서버의 구조화된 로그에 남긴다.

issuer가 바뀌면(바로 바꾸든, 대기 설정을 적용하든) 옛 공급자는 `oidc_former_provider_days` 동안 ‘연결용’으로 남는다. 옛 공급자로는 로그인되지 않는다: 그 issuer로 만든 세션은 옮기는 순간 끝나고, 옛 공급자의 로그인은 세션을 만들지 않는다. 그동안 새 공급자로 로그인했는데 그 신원이 어느 계정에도 연결돼 있지 않은 사람은 계정이 바로 만들어지지 않고, 로그인 화면에서 ‘이미 계정이 있음(예전 로그인으로 잇기)’과 ‘새로 시작’ 가운데 고른다. 서버는 그 로그인을 그 브라우저의 쿠키로만 찾을 수 있게 10분 동안 잡아 두고(쿠키의 비밀 값은 해시로만 저장), 한 번만 쓴다. 잇기는 옛 공급자로 한 번 로그인하는 것이고, 옛 공급자가 확인해 준 신원이 이 조직에 자리가 있는 계정(구성원, 또는 쓰이는 워크스페이스의 손님)에 연결돼 있으면 새 신원을 그 계정에 연결하고 새 공급자로 만든 세션으로 들어온다. 두 신원을 모두 로그인으로 증명하므로 이메일은 보지 않는다. 잡아 둔 것이 없거나 끝났거나 다른 브라우저의 것이거나, 연결용 기간이 끝났거나, 옛 신원의 계정이 없거나 이 조직에 자리가 없거나, 새 신원이 그사이 다른 계정에 연결됐으면 아무것도 바뀌지 않고 모두 같은 답(`oidc_carry_over_refused`)을 받는다. 새로 시작은 첫 로그인의 규칙 그대로다(자동 가입이 꺼져 있으면 초대가 있어야 한다). 조직 관리자는 SSO 절에서 연결용 기간이 끝나는 때를 보고 일찍 끝낼 수 있으며, 기간이 지나면 서버가 지운다. 연결용으로 남김, 일찍 끝냄, 기간이 끝남(행위자 ‘서버’), 관리자가 연결한 신원, 구성원이나 손님이 이은 신원은 issuer와 함께 조직의 감사 기록에 남는다. 연결하거나 이은 신원의 기록에는 그 신원이 누구인지도 남는다: 새 공급자가 준 이메일(확인하지 않은 값), 없으면 subject. 조직의 기록은 조직 관리자만 읽고, 기록은 고쳐지지 않으므로 그 값은 계정이 없어진 뒤에도 남는다. 다시 옮기면 그 전 공급자가 연결용이 되고, 먼저 남아 있던 것은 끝난다.

공급자로 만든 세션은 그 공급자의 조직과 issuer, 그리고 공급자가 준 subject를 적어 둔다. 공급자를 지우거나 다른 issuer로 바꾸면 그 조직에서 그 issuer로 만든 세션이 모두 끝나고, 계정에서 신원을 끊으면 그 신원(issuer와 subject)으로 만든 그 사람의 세션이 끝난다(끊는 요청을 보낸 세션도 그렇게 만든 것이면 함께). 같은 issuer의 다른 신원으로 만든 세션은 남는다. client ID만 바꾸면 사람의 신원이 바뀌지 않으므로 세션을 끝내지 않는다. 끝난 세션의 WebSocket 연결은 곧바로 닫히고 카드 본문의 방에서도 나간다. 마이그레이션 0028은 공급자가 남은 옛 세션에 issuer를 채우고, 공급자가 없어진 옛 OIDC 세션은 끝낸다. 0029 전에 만든 세션은 subject를 모르므로 그 issuer의 어느 신원을 끊어도 끝난다.

## 명령줄 인자

| 인자 | 기본값 | 뜻 |
|---|---|---|
| `--config <경로>` | `./config.json` | 설정 파일 |
| `--port <포트>` | — | `port`를 바꾼다. 0이면 무시한다 |
| `--database-kind <종류>` | — | `database_kind`를 바꾼다. 비면 무시한다 |
| `--database-connection <문자열>` | — | `database_connection`을 바꾼다. 비면 무시한다. 로그에 값을 남기지 않는다 |
| `--print-openapi` | — | 계약(OpenAPI 문서)을 표준 출력에 쓰고 끝난다. 설정을 읽지 않는다 |
| `--migrate` | — | 데이터베이스의 스키마를 이 서버의 것으로 올리고(`database_connection`의 역할로, 서버 잠금을 쥔 채) 끝난다. 아무것도 띄우지 않고, 역할이 벽을 지나는지(슈퍼유저) 보지 않는다. 서버가 표의 주인이 아닌 역할로 붙을 때 표의 주인(DB의 관리자)으로 먼저 돌린다(위 "키: 데이터베이스") |
| `--migrate --check` | — | `--migrate`가 무엇을 할지(그대로 둠, 기록만 기준선의 것으로 바꿈, 밀린 이주를 올림) 로그에 말하고 아무것도 바꾸지 않고 끝난다. 거절될 데이터베이스면 실패로 끝난다. 서버 잠금을 잡지 않으므로 서버가 도는 동안에도 돌린다. `docker/update.sh`가 멈추기 전에 쓴다 |
| `--check-ready` | — | 설정이 말하는 이 기기의 서버(포트, `serve_https`, `FVOCI_BASE_PATH`)에 `GET /api/v1/ready`를 물어(3초까지) 200이면 성공으로, 아니면 실패로 끝난다. 아무것도 열거나 띄우지 않는다. 컨테이너의 상태 검사가 쓴다 |
| `--help`, `--version` | — | 도움말과 판 |

## 그 밖의 환경 변수

| 변수 | 뜻 |
|---|---|
| `FVOCI_BASE_PATH` | 웹앱과 API를 놓을 경로. 비거나 `/boards` 같은 절대 경로여야 한다 |
| `FVOCI_LOG_LEVEL` | 로그의 수준과 거르개(`tracing`의 `EnvFilter` 문법, 예: `info`, `fvoci_server=debug`). 기본은 `info` |
| `FVOCI_LOG_FORMAT` | `json`이면 로그를 줄마다 JSON으로 쓴다. 그 밖에는 사람이 읽는 꼴이다 |
| `FVOCI_SECRET_KEY` | 서버가 DB에 봉인해 두는 값(조직 공급자의 client secret)의 키. 32바이트를 표준 base64로 쓴 것이다(`openssl rand -base64 32`). AES-256-GCM의 키로 쓴다. 환경 변수로만 받는다: 설정 파일에 `secret_key`가 있으면 시작하지 않는다. 꼴이 틀리면 값을 되풀이하지 않는 오류로 시작하지 않는다. 로그와 `Debug` 출력에 나오지 않는다. 없으면 공급자를 저장할 수 없다(409, `secret-key-required`). 바꾸면 저장된 secret이 열리지 않으므로 공급자마다 secret을 다시 저장한다 |
| `FVOCI_BUILD_HASH`, `FVOCI_BUILD_DATE` | 서버를 빌드할 때만 읽는다: 빌드한 커밋과 그 시각. `ping`의 `commit`·`buildDate`가 이것이고, 주지 않으면 빈 글이다. 이미지는 같은 이름의 빌드 인자로 받고 `cargo xtask docker`가 넘긴다(고치지 않은 것이 있는 작업 트리면 커밋 뒤에 `-dirty`) |
| `AWS_*` | `file_storage_s3.access_key_id`가 비었을 때 S3 클라이언트가 읽는 자격과 설정(위 "S3의 예") |

## 예시 파일

`docker/fvoci/config.json`이 컨테이너 이미지에 들어가는 설정이다. 테스트 `the_example_configuration_files_are_read`가 이 파일을 읽고 검사해 오류가 없는지 본다.
