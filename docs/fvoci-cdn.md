# fvoci-server를 CDN 뒤에 두기: CloudFront와 Cloudflare

`fvoci-server`에는 CDN 전용 코드가 없다. 표준 HTTP 헤더(`Cache-Control`, `ETag`, `Last-Modified`, `Vary`, `Content-Encoding`)만 보내므로, CDN이 그 헤더를 따르도록 설정하면 된다. 이 문서는 서버가 보내는 응답과, 그에 맞춘 Amazon CloudFront, Cloudflare 설정을 정리한다.

CloudFront와 Cloudflare의 수치, 정책 이름은 2026-11-10에 각 벤더 공식 문서에서 확인했고 출처를 옆에 적었다. 확인하지 못한 값은 확인하지 못했다고 적었다. 이 구성 전체를 실제 CDN에 통과시켜 시험한 것은 아니다(마지막 장 참고).

## 1. 경로별 요약

| 경로 | 서버 응답 | CDN 캐시 | CDN이 오리진으로 전달해야 하는 것 |
| --- | --- | --- | --- |
| `/static/*`, 이름에 콘텐츠 해시가 있는 파일 | `Cache-Control: public, max-age=31536000, immutable` | 캐시한다. TTL은 오리진 헤더를 따른다. | 메서드 `GET`, `HEAD`. 헤더 `Accept-Encoding`. 쿼리 문자열과 쿠키는 필요 없다. |
| `/static/*`, 그 밖의 파일 | `Cache-Control: no-cache`, `ETag`, `Last-Modified` | 저장하더라도 매번 오리진에 재검증한다. | 위와 같고, 조건부 요청 헤더(`If-None-Match`, `If-Modified-Since`). |
| `/api/v1/*` | `Cache-Control: no-store` | 캐시하지 않는다. | 메서드 `GET`, `HEAD`, `OPTIONS`, `PUT`, `POST`, `PATCH`, `DELETE`. 뷰어 헤더 전부(최소 `Cookie`, `Origin`, `X-Requested-With`, `Content-Type`, 그리고 브라우저가 아닌 클라이언트가 쓰는 `Authorization`). 쿼리 문자열 전부(공유 보드의 `readToken` 포함). 쿠키 `__Host-fvoci_session`(HTTP로 서비스하면 `fvoci_session`). 응답의 `Set-Cookie`와 `Retry-After`를 그대로 돌려준다. |
| `/api/v1/w/{workspace}/containers/{container}/files/{file}`, `/api/v1/shared/{container}/files/{file}` | `Cache-Control: private, no-cache`, `X-Content-Type-Options: nosniff`, `Content-Security-Policy: sandbox; frame-ancestors 'none'`, `Range` 지원 | 캐시하지 않는다. | 위와 같고 `Range`, `If-Range`. |
| `/ws` | WebSocket(`101 Switching Protocols`) | 캐시하지 않는다. | 메서드 `GET`. 헤더 `Upgrade`, `Connection`, `Sec-WebSocket-*`, 그리고 `Origin`과 `Cookie`. 인증은 핸드셰이크에서 세션 쿠키로 하고, `Origin`이 서버 자신의 것(`server_url`)이나 `allowed_origins`가 아니면 403으로 거절한다. CDN이 `Origin`을 고쳐 쓰거나 쿠키를 떼면 실시간 갱신이 되지 않는다. |
| 그 밖의 모든 경로 | 웹 앱의 `index.html`, 상태 200, `Cache-Control: no-cache`. `/problems`와 그 아래는 오류의 종류를 설명하는 글(`text/plain`, `Cache-Control: public, max-age=3600`)이다 | 캐시하지 않거나 매번 재검증한다. `/problems`는 캐시해도 된다. | 메서드 `GET`, `HEAD`. 그 외에는 필요 없다. |

- 정적 파일 옆에 `<file>.br`이나 `<file>.gz`가 있고 클라이언트가 받아들이면, 서버는 미리 압축한 파일을 `Content-Encoding`, `Vary: Accept-Encoding`과 함께 보낸다. CDN 캐시 키에 `Accept-Encoding`이 반영되어야 한다.
- 무언가를 바꾸는 API 요청(`GET`, `HEAD`, `OPTIONS`가 아닌 것)에는 `X-Requested-With: XMLHttpRequest`가 있어야 한다. 없으면 서버가 403(`reason`이 `csrf`)으로 답한다. `Origin` 헤더가 있으면 그 값이 서버 자신의 것이거나 `allowed_origins`에 있어야 하고, 아니면 403(`foreign_origin`)이다. CDN이 `X-Requested-With`를 떼거나 `Origin`을 바꾸면 로그인을 포함해 쓰는 요청 전체가 403이 된다. `Authorization: Bearer`를 단 요청은 `X-Requested-With` 없이도 받는다.
- 세션은 로그인 응답의 `Set-Cookie`로 간다: `HttpOnly`, `SameSite=Lax`, `Path=/`이고, `server_url`이 `https`면 이름이 `__Host-fvoci_session`이고 `Secure`가 붙는다(`http`면 `fvoci_session`). 응답 본문에는 토큰이 없다. 브라우저가 아닌 클라이언트는 같은 토큰을 `Authorization: Bearer <token>`으로 보낼 수 있다. 쿼리 문자열로는 세션 토큰을 받지 않는다. 공유 보드는 쿼리 매개변수 `readToken`을 쓴다.
- `__Host-` 쿠키는 브라우저가 `Secure`, `Path=/`, `Domain` 없음일 때만 받는다. CDN이나 프록시가 `Set-Cookie`에 `Domain`을 더하거나 `Path`를 고치면 브라우저가 쿠키를 버려 로그인이 되지 않는다.
- 첨부 파일 경로는 `.png`, `.pdf` 같은 확장자로 끝날 수 있다. 확장자 기준으로 캐시하는 CDN에서는 `/api/*`를 명시적으로 캐시에서 빼야 한다.
- 경로 접두사 아래에서 서비스하면(2장) 이 문서의 모든 경로 패턴 앞에 접두사가 붙는다. 예: `/boards/static/*`, `/boards/api/*`, `/boards/ws`.

## 2. 서버 설정

| 설정 | 프록시 뒤에서의 의미 |
| --- | --- |
| `server_url` | 공개 URL. 예: `https://boards.example.com`. 오리진 주소가 아니라 CDN에 연결한 도메인을 적는다. base path도 이 값에서 정해진다. 서버는 이 값을 `index.html`에 채워 보낸다. |
| `FVOCI_BASE_PATH` (환경 변수) | 경로 접두사 아래에서 서비스할 때 쓴다. |
| `port`, `listen_loopback_only` | 수신 포트. `listen_loopback_only`가 true면 `localhost`에서만 받는다. 같은 호스트의 프록시나 터널 뒤에 둘 때 쓴다. |
| `serve_https` | TLS는 보통 CDN에서 끝난다. 오리진은 사설망 안에서 HTTP로 두거나 `serve_https`로 HTTPS를 쓴다. `serve_https`를 켰는데 `./cert/cert.pem`이나 `./cert/key.pem`이 없으면 서버는 뜨지 않는다(평문으로 떨어지지 않는다). |
| `trusted_proxies` | 기본값은 빈 목록. 서버 앞에 있는 프록시의 주소와 주소 범위(`10.0.0.0/8` 같은)를 적는다. 그 주소에서 온 요청만 `X-Forwarded-For`를 믿고, 목록을 오른쪽에서부터 읽어 믿는 프록시가 아닌 첫 주소를 클라이언트로 본다. 요청 로그와 로그인 시도 제한이 같은 주소를 쓴다. |
| `allowed_origins` | 기본값은 빈 목록. `server_url` 말고도 쓰는 요청과 WebSocket 핸드셰이크를 받을 출처(`https://host[:port]`). |
| `session_idle_days`, `session_max_days` | 기본값 14, 90. 쿠키의 `Max-Age`는 한가함 한도와 같다. |
| `websocket_ping_seconds` | 기본값 30, 1–59. 서버가 이 간격으로 ping 프레임을 보낸다. |
| `max_file_bytes` | 서버 자체의 업로드 한도(바이트). 기본값 100 MiB. 0이나 음수는 시작 오류다. CDN의 한도와 별개로 적용된다. 큰 파일은 청크(`upload_part_bytes`, 기본 8 MiB)로 나눠 오므로 CDN의 요청 본문 한도는 부분 하나보다 크면 된다. |

- 서버가 로그인 시도를 제한한다: (계정, 주소) 쌍의 연속 실패, 계정 전체의 연속 실패, 주소당 분당 시도 수, 그리고 비밀번호 해시의 대기열. 넘으면 429와 `Retry-After`로 답한다(설정 `login_pair_*`, `login_account_*`, `login_address_attempts_per_minute`, `password_hash_*`). 주소별 제한은 서버가 보는 클라이언트 주소에 달렸으므로, CDN 뒤에서는 `trusted_proxies`를 맞게 적어야 한다. 비워 두면 모든 요청이 CDN의 주소에서 온 것으로 보여 주소당 제한이 모든 사용자에게 한꺼번에 걸리고, (계정, 주소) 쌍의 기다림도 주소가 하나라 한 사람이 틀린 계정을 모두가 기다린다. `server_url`이 https이고 `serve_https`가 꺼져 있는데 `trusted_proxies`가 비어 있으면 서버가 시작할 때 경고를 낸다. `X-Forwarded-For`의 항목은 `주소:포트` 꼴도 읽고, `Forwarded` 헤더(RFC 7239)는 읽지 않는다. CDN이나 WAF의 rate limiting(3.5, 4.4)은 그 앞의 한 겹으로 둔다.
- 클라이언트가 `X-Forwarded-For`를 직접 보내면 CloudFront는 뷰어 주소를 그 값의 끝에 덧붙이고([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/RequestAndResponseBehaviorCustomOrigin.html), 2026-11-10 확인), Cloudflare도 기존 값에 덧붙인다([출처](https://developers.cloudflare.com/fundamentals/reference/http-headers/), 2026-11-10 확인). 따라서 목록의 왼쪽 끝은 클라이언트가 보낸 값일 수 있고, 서버는 그것을 믿지 않는다: 오른쪽에서부터 `trusted_proxies`에 있는 주소를 지나 처음 만나는 주소를 쓴다. CDN의 주소 범위가 바뀌면 `trusted_proxies`도 바꿔야 한다(범위가 빠지면 그 CDN 노드가 클라이언트로 보인다).
- 오리진에는 CDN만 접근할 수 있게 막는다. 오리진이 직접 열려 있으면 CDN의 WAF 규칙을 우회할 수 있다.

## 3. Amazon CloudFront

### 3.1 동작(cache behavior)

CloudFront는 동작 목록의 순서대로 경로 패턴을 비교해 처음 맞는 동작을 적용하고, 기본 동작 `*`는 항상 마지막에 처리한다. 경로 패턴은 대소문자를 구분하고, 쿼리 문자열은 비교에 쓰지 않는다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/DownloadDistValuesCacheBehavior.html), 2026-11-10 확인).

| 순서 | Path pattern | Cache policy | Origin request policy | Allowed HTTP methods |
| --- | --- | --- | --- | --- |
| 1 | `/api/*` | `CachingDisabled` | `AllViewer` 또는 `AllViewerExceptHostHeader` | GET, HEAD, OPTIONS, PUT, POST, PATCH, DELETE |
| 2 | `/ws` | `CachingDisabled` | `AllViewer` 또는 `AllViewerExceptHostHeader` | GET, HEAD |
| 3 | `/static/*` | 아래의 사용자 정의 정책 또는 `UseOriginCacheControlHeaders` | 없음 | GET, HEAD |
| 4 | `*` (기본 동작) | 3번과 같은 정책 또는 `CachingDisabled` | 없음 | GET, HEAD |

Allowed HTTP methods에서 고를 수 있는 조합은 `GET, HEAD` / `GET, HEAD, OPTIONS` / `GET, HEAD, OPTIONS, PUT, POST, PATCH, DELETE` 세 가지이고, CloudFront는 `GET`, `HEAD`(선택하면 `OPTIONS`) 응답만 캐시한다(같은 출처). Viewer protocol policy는 `/api/*`와 `/ws`에 `HTTPS only`를 권한다.

### 3.2 관리형 정책

정책 이름과 ID의 출처: [managed cache policies](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/using-managed-cache-policies.html), [managed origin request policies](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/using-managed-origin-request-policies.html) (2026-11-10 확인).

| 정책 | ID | 내용 |
| --- | --- | --- |
| `CachingDisabled` | `4135ea2d-6df8-44a3-9df3-4b5a84be39ad` | Minimum, Default, Maximum TTL이 모두 0초. 캐시 키에 헤더, 쿠키, 쿼리 문자열이 없다. |
| `UseOriginCacheControlHeaders` | `83da9c7e-98b4-4e11-a168-04f0df8e2c65` | Minimum TTL 0초, Default TTL 0초, Maximum TTL 31,536,000초. 캐시 키에 `Host`, `Origin`, `X-HTTP-Method-Override`, `X-HTTP-Method`, `X-Method-Override`, 모든 쿠키, 정규화한 `Accept-Encoding`이 들어간다. |
| `CachingOptimized` | `658327ea-f89d-4fab-a63d-7e88639e58f6` | Minimum TTL 1초, Default TTL 86,400초, Maximum TTL 31,536,000초. 캐시 키에는 정규화한 `Accept-Encoding`만 들어간다. |
| `AllViewer` | `216adef6-5c7f-47e4-b989-5492eafa07d3` | 뷰어 요청의 헤더, 쿠키, 쿼리 문자열을 모두 오리진에 보낸다. |
| `AllViewerExceptHostHeader` | `b689b0a8-53d0-40ab-baf2-68738e2966ac` | `Host`만 빼고 모두 보낸다. CloudFront가 `Host`를 오리진 도메인 이름으로 채운다. |

**`/static/*`.** 오리진 헤더를 따르고 `Accept-Encoding`을 정규화하는 정책이 필요하다. 선택지는 세 가지다.

- 사용자 정의 cache policy(권장): Minimum TTL 0, Default TTL 0, Maximum TTL 31536000, 헤더, 쿠키, 쿼리 문자열 없음, Gzip과 Brotli 켬. 캐시 키가 가장 작다. Minimum TTL이 0이면 CloudFront는 오리진의 `no-cache`, `no-store`, `private`을 따른다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/Expiration.html), 2026-11-10 확인).
- `UseOriginCacheControlHeaders`: TTL은 같지만 모든 쿠키가 캐시 키에 들어간다. 브라우저가 세션 쿠키를 같은 호스트의 모든 요청에 보내므로 사용자마다 따로 캐시되어 적중률이 떨어진다.
- `CachingOptimized`: 캐시 키는 작지만 Minimum TTL이 1초라서 `no-cache`인 파일도 최소 1초 동안 캐시한다. AWS 문서가 이 점을 경고한다.

압축 설정(Gzip, Brotli)을 켠 cache policy는 정규화한 `Accept-Encoding`을 캐시 키와 오리진 요청에 넣는다. 오리진 응답에 `Content-Encoding`이 있으면 CloudFront는 다시 압축하지 않고 그대로 보내고 캐시한다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/ServingCompressedFiles.html), 2026-11-10 확인). 서버가 미리 압축한 `.br`, `.gz` 파일이 그대로 쓰인다.

**기본 동작 `*`.** `index.html`은 `no-cache`다. Minimum TTL이 0인 정책(위의 사용자 정의 정책, `UseOriginCacheControlHeaders`)이나 `CachingDisabled`를 쓴다. Custom error response로 403이나 404를 `/index.html`로 바꾸는 설정은 넣지 않는다. 서버가 이미 `index.html`을 200으로 답하고, 그 설정은 API의 오류 응답까지 HTML로 바꾼다.

**`/api/*`, `/ws`.** `CachingDisabled`와 함께 모든 뷰어 값을 전달한다.

- CloudFront는 기본적으로 `GET`, `HEAD` 요청에서 `Authorization` 헤더를 떼고 오리진에 보낸다. 모든 뷰어 헤더를 전달하는 origin request policy를 쓰면 전달된다. `Authorization`을 캐시 키에 넣지 않고 전달할 때는 캐시를 꺼야 한다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/add-origin-custom-headers.html), 2026-11-10 확인).
- `AllViewer`는 뷰어의 `Host`(공개 도메인)를 오리진에 그대로 보낸다. 직접 운영하는 서버나 로드 밸런서가 오리진일 때 쓴다. 오리진과 HTTPS로 통신하면 오리진 인증서가 뷰어의 `Host`와 맞아야 한다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/DownloadDistValuesOrigin.html), 2026-11-10 확인).
- `AllViewerExceptHostHeader`는 오리진이 자기 도메인 이름을 `Host`로 받아야 동작할 때 쓴다. AWS 문서는 API Gateway, Lambda function URL 오리진을 대상으로 든다. 오리진 앞단이 `Host`로 라우팅하는 경우가 여기에 해당한다.
- 파일 다운로드의 `Range` 헤더는 CloudFront가 오리진에 전달한다([출처](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/RangeGETs.html), 2026-11-10 확인).

### 3.3 WebSocket

출처: [WebSocket 문서](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/distribution-working-with.websockets.html), [Quotas](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cloudfront-limits.html) (2026-11-10 확인).

- WebSocket은 모든 배포에서 자동으로 켜져 있다. 동작에서 모든 뷰어 헤더를 전달하거나(`AllViewer`), 최소한 `Sec-WebSocket-Key`와 `Sec-WebSocket-Version`을 전달해야 한다. 문서가 이름을 든 관리형 정책은 `AllViewer`다.
- CloudFront는 WebSocket을 HTTP/1.1로만 지원한다.
- 유휴 타임아웃은 10분이다. 오리진에서 클라이언트로 가는 바이트가 10분 동안 없으면 연결을 닫는다. 서버의 ping(`websocket_ping_seconds`, 1–59초, 기본 30초)이 오리진에서 나가는 바이트이므로 닫히지 않는다.

### 3.4 타임아웃과 요청 본문 크기

| 항목 | 값 | 출처 (2026-11-10 확인) |
| --- | --- | --- |
| Response timeout (origin read timeout) | 기본 30초. 설정 범위 1~120초, 그 이상은 할당량 상향 요청 | [Origin settings](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/DownloadDistValuesOrigin.html), [Quotas](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cloudfront-limits.html) |
| Response completion timeout | 설정하지 않으면 상한이 없다. 설정하면 response timeout 이상이어야 한다. | [Origin settings](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/DownloadDistValuesOrigin.html) |
| Keep-alive timeout | 기본 5초. 설정 범위 1~300초 | 위 두 문서 |
| HTTP 요청 본문 최대 크기 | 64 GB | [Quotas](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cloudfront-limits.html) |
| 요청 길이(경로, 쿼리 문자열, 헤더 포함, 본문 제외) | 32,768바이트. URL은 8,192바이트 | [Quotas](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cloudfront-limits.html) |

- Response timeout은 CloudFront가 요청을 오리진에 넘긴 뒤 응답을 기다리는 시간이자, 응답 패킷 사이의 최대 간격이다. `POST`, `PUT`, `PATCH`, `DELETE`, `OPTIONS` 요청은 시간이 지나면 연결을 끊고 재시도하지 않는다.
- 아카이브 가져오기(`POST /api/v1/w/{workspaceId}/archive/import`)처럼 서버가 본문을 다 받은 뒤 응답하기까지 30초를 넘길 수 있는 요청은 response timeout을 늘려야 한다. 넘기면 CloudFront가 504로 답한다.
- 업로드 본문을 전송하는 시간이 response timeout에 포함되는지는 문서에서 확인하지 못했다. 큰 파일로 직접 시험해야 한다.
- Keep-alive timeout이 효과를 내려면 오리진이 지속 연결을 허용해야 한다.
- CloudFront의 본문 한도는 크므로, 실제 한도는 서버의 `max_file_bytes`와 중간 프록시 설정이 정한다.

### 3.5 AWS WAF rate-based rule 예시

아래는 예시다. 한도, 창, 동작은 배포에 맞게 고친다. 같은 IP에서 5분 동안 두 경로로 보내는 `POST`가 20회를 넘으면 차단한다. base path를 쓰면 `SearchString`에 접두사를 붙인다. 콘솔의 Rule JSON editor에서 검증한 뒤 쓴다.

```json
{
  "Name": "fvoci-login-rate",
  "Priority": 0,
  "Action": { "Block": {} },
  "VisibilityConfig": { "SampledRequestsEnabled": true, "CloudWatchMetricsEnabled": true, "MetricName": "fvoci-login-rate" },
  "Statement": { "RateBasedStatement": {
    "Limit": 20,
    "EvaluationWindowSec": 300,
    "AggregateKeyType": "IP",
    "ScopeDownStatement": { "AndStatement": { "Statements": [
      { "ByteMatchStatement": { "FieldToMatch": { "Method": {} }, "PositionalConstraint": "EXACTLY",
          "SearchString": "POST", "TextTransformations": [ { "Type": "NONE", "Priority": 0 } ] } },
      { "OrStatement": { "Statements": [
        { "ByteMatchStatement": { "FieldToMatch": { "UriPath": {} }, "PositionalConstraint": "EXACTLY",
            "SearchString": "/api/v1/login", "TextTransformations": [ { "Type": "NONE", "Priority": 0 } ] } },
        { "ByteMatchStatement": { "FieldToMatch": { "UriPath": {} }, "PositionalConstraint": "EXACTLY",
            "SearchString": "/api/v1/register", "TextTransformations": [ { "Type": "NONE", "Priority": 0 } ] } }
      ] } }
    ] } }
  } }
}
```

- `Limit`의 최솟값은 10이다. `EvaluationWindowSec`는 60, 120, 300, 600 중 하나이고 기본값은 300이다([출처](https://docs.aws.amazon.com/waf/latest/developerguide/waf-rule-statement-type-rate-based-high-level-settings.html), 2026-11-10 확인).
- 한도는 정확히 지켜지지 않는다. 초과를 감지해 동작을 적용하기까지의 지연은 보통 30초 미만이고, 몇 분이 걸릴 수도 있다([출처](https://docs.aws.amazon.com/waf/latest/developerguide/waf-rule-statement-type-rate-based-caveats.html), 2026-11-10 확인).
- `AWSManagedRulesCommonRuleSet`을 같이 쓰면 `SizeRestrictions_BODY` 규칙이 8 KB(8,192바이트)를 넘는 요청 본문을 차단한다([출처](https://docs.aws.amazon.com/waf/latest/developerguide/aws-managed-rule-groups-baseline.html), 2026-11-10 확인). 파일 업로드와 아카이브 가져오기 경로는 이 규칙에서 제외해야 한다.

## 4. Cloudflare

### 4.1 Cache Rules

Cloudflare의 기본 동작([출처](https://developers.cloudflare.com/cache/concepts/default-cache-behavior/), [Origin Cache Control](https://developers.cloudflare.com/cache/concepts/cache-control/), 2026-11-10 확인):

- 캐시 대상은 MIME 형식이 아니라 파일 확장자로 정한다(`JS`, `CSS`, `PNG`, `SVG`, `WOFF2`, `PDF`, `ZIP` 등). HTML과 JSON은 기본으로 캐시하지 않는다. 따라서 `index.html`과 API의 JSON 응답은 규칙이 없어도 캐시되지 않는다.
- 응답의 `Cache-Control`이 `private`, `no-store`, `no-cache`, `max-age=0`이면 캐시하지 않는다.
- Origin Cache Control은 Free, Pro, Business 플랜에서 기본으로 켜져 있고 끌 수 없다. Enterprise는 선택할 수 있다. 켜져 있으면 오리진의 `Cache-Control` 지시문을 그대로 따른다. `no-cache`는 저장하되 항상 재검증하고, `immutable`은 브라우저로 전달한다.

기본 동작만으로도 대체로 맞지만, 첨부 파일 경로가 캐시 대상 확장자로 끝날 수 있으므로 아래 규칙을 명시한다. 옵션 이름의 출처: [Cache Rules settings](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/) (2026-11-10 확인).

| 규칙 | 식(expression) | 설정 |
| --- | --- | --- |
| API와 WebSocket 우회 | `starts_with(http.request.uri.path, "/api/") or http.request.uri.path eq "/ws"` | Cache eligibility: **Bypass cache** |
| 정적 파일 | `starts_with(http.request.uri.path, "/static/")` | Cache eligibility: **Eligible for cache**. Edge TTL: **Use cache-control header if present, bypass cache if not**. Browser TTL: **Respect origin** |

- Edge TTL과 Browser TTL을 "Ignore cache-control header and use this TTL"이나 override로 두지 않는다. 그러면 `no-cache`인 파일이 오래 남는다.
- 모든 경로를 "Eligible for cache"로 만드는 규칙을 추가하려면 우회 규칙이 `/api/*`와 `/ws`에 계속 적용되는지 확인한다.
- Cloudflare는 오리진에 `accept-encoding: br, gzip`으로 요청하고, 오리진이 `content-encoding`과 함께 압축된 응답을 주면 방문자가 그 방식을 지원하고 응답을 바꾸는 기능이 꺼져 있을 때 같은 압축을 유지한다([출처](https://developers.cloudflare.com/speed/optimization/content/compression/), 2026-11-10 확인). 서버가 미리 압축한 파일이 쓰인다.
- Cloudflare에는 CloudFront의 origin request policy에 해당하는 전달 설정이 없어서 `Origin`, `X-Requested-With`, `Authorization`, 쿼리 문자열, 쿠키를 위한 별도 설정을 적지 않았다. 이 전달 동작은 벤더 문서에서 따로 확인하지 않았으므로 5장의 4번 점검으로 확인한다. 요청 헤더를 지우는 Transform Rule을 쓰고 있다면 이 헤더들이 남는지 본다.

### 4.2 WebSocket

출처: [WebSockets](https://developers.cloudflare.com/network/websockets/) (2026-11-10 확인).

- WebSocket은 모든 플랜에서 지원한다. 대시보드 Network 페이지의 **WebSockets** 토글이 On인지 확인한다. 문서는 추가 설정 없이 프록시된 WebSocket을 지원한다고 적지만 토글의 기본값은 명시하지 않는다.
- 양방향으로 데이터가 없는 상태가 일정 시간 이어지면 Cloudflare가 연결을 닫는다. 이 유휴 시간의 값은 문서에 적혀 있지 않아 확인하지 못했다. Enterprise는 담당 팀을 통해 값을 바꿀 수 있다. 서버의 ping(`websocket_ping_seconds`, 기본 30)이 그보다 짧은지 본다.
- Cloudflare가 자사 네트워크에 새 코드를 배포하면서 서버를 재시작하면 WebSocket 연결이 끊긴다. 가끔 생기는 재연결은 정상이다.

### 4.3 업로드 크기와 타임아웃

| 항목 | 값 | 출처 (2026-11-10 확인) |
| --- | --- | --- |
| 최대 업로드 크기(요청 본문) | Free 100 MB, Pro 100 MB, Business 200 MB, Enterprise 기본 500 MB이고 Network 페이지의 Maximum Upload Size에서 5 GB까지 직접 조정. 넘으면 `413` | [Error 413](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/4xx-client-error/error-413/), [Enterprise 기본값](https://developers.cloudflare.com/changelog/post/2026-09-04-enterprise-self-serve-upload-limits/) |
| Proxy Read Timeout | 125초. 넘으면 `524`. Enterprise만 조정 가능(최대 6,000초) | [Connection limits](https://developers.cloudflare.com/fundamentals/reference/connection-limits/), [Error 524](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-5xx-errors/error-524/) |
| Proxy Write Timeout | 30초. 넘으면 `524`. 조정할 수 없다. | 위 두 문서 |

- 파일과 가져올 아카이브는 부분(`upload_part_bytes`, 기본 8 MiB)으로 나눠 오므로 Cloudflare의 요청 본문 한도는 부분 하나에만 걸린다. `upload_part_bytes`를 플랜 한도보다 작게 둔다. 그러면 `max_file_bytes`와 `max_import_bytes`는 100 MB를 넘겨도 된다.
- 남는 것은 시간이다. 업로드를 마치는 요청이나 아카이브 가져오기의 처리가 기본 타임아웃인 125초를 넘기면 `524`로 끝난다. Cloudflare 문서가 드는 방법처럼 그 요청만 DNS-only 호스트 이름으로 보낼 수는 없다. `https://` `server_url`에서 브라우저의 세션 쿠키는 그 호스트에만 붙는 `__Host-fvoci_session`이고, 바꾸는 요청은 `Origin`을 `server_url`과 맞춰 본다. 그런 배포는 `server_url`의 호스트 자체를 Cloudflare 프록시 밖에 두거나(DNS-only, 또는 오리진 앞 리버스 프록시로 바로), Enterprise에서 Proxy Read Timeout을 올린다.
- 한도를 올려도 아주 큰 업로드는 크기 한도에 닿기 전에 연결 타임아웃이나 read timeout으로 실패할 수 있다고 Cloudflare 문서가 적고 있다.

### 4.4 Rate limiting rule 예시

아래는 예시다. 임계값과 동작은 배포에 맞게 고친다.

| 항목 | 값 |
| --- | --- |
| When incoming requests match | `(http.request.uri.path in {"/api/v1/login" "/api/v1/register"} and http.request.method eq "POST")` |
| With the same characteristics | IP |
| When rate exceeds | Free: 5 requests / 10 seconds. Pro 이상 예: 10 requests / 1 minute |
| Then take action | Block |
| For duration | Free: 10 seconds. Pro 이상 예: 10 minutes |

플랜별 제약([출처](https://developers.cloudflare.com/waf/rate-limiting-rules/), 2026-11-10 확인):

| | Free | Pro | Business |
| --- | --- | --- | --- |
| 규칙 수 | 1 | 2 | 5 |
| 집계 기간 | 10초 | 최대 1분 | 최대 10분 |
| 차단 유지 시간 | 10초 | 최대 1시간 | 최대 1일 |
| 식에 쓸 수 있는 필드 | Path, Verified Bot | Host, URI, Path, Full URI, Query, Verified Bot | 앞의 것에 Method, Source IP, User Agent 추가 |

- Free 플랜은 규칙이 하나뿐이라 두 경로를 한 식에 넣었다. Free에서는 10초 단위로만 셀 수 있어 느린 대입 공격은 막지 못한다.
- 위 표로는 `http.request.method`를 Business 이상에서만 쓸 수 있다. 그런데 Cloudflare의 다른 [안내 문서](https://developers.cloudflare.com/use-cases/solutions/stop-account-takeover-attacks/)는 같은 식을 Free 플랜 절차에 쓴다. 어느 쪽이 맞는지 확인하지 못했다. 대시보드가 식을 거부하면 `and http.request.method eq "POST"`를 뺀다. 이 두 경로는 `POST`만 쓰므로 결과는 거의 같다.
- Enterprise의 한도는 계약에 따라 다르므로 여기에 적지 않았다.

## 5. 배포 확인

`BASE`는 공개 URL, `JAR`는 `curl`이 쿠키를 두는 파일이다. 보드와 파일의 이름은 실제 값으로 바꾼다.

```sh
BASE=https://boards.example.com
JAR=$(mktemp)

# 1. 해시가 있는 정적 파일. 두 번 실행한다.
curl -sI -H 'Accept-Encoding: br' "$BASE/static/<해시가-있는-파일>.js"
#    200, Cache-Control: public, max-age=31536000, immutable
#    .br 파일이 있으면 Content-Encoding: br, Vary: Accept-Encoding
#    두 번째 응답에 캐시 적중 표시가 있어야 한다. CloudFront는 X-Cache, Cloudflare는 CF-Cache-Status 헤더
#    (헤더 이름과 값은 이번에 벤더 문서에서 확인하지 않았다)

# 2. 해시가 없는 정적 파일
curl -sI "$BASE/static/<해시가-없는-파일>"
#    200, Cache-Control: no-cache, ETag, Last-Modified

# 3. index.html. API도 정적 파일도 아닌 임의의 경로
curl -sI "$BASE/any/client/route"
#    200, Content-Type: text/html, Cache-Control: no-cache. 캐시 적중 표시가 없어야 한다.

# 4. API
#    로그인해 쿠키를 받는다. 204이고 Set-Cookie가 그대로 와야 한다(__Host-fvoci_session, Secure, HttpOnly, SameSite=Lax, Path=/).
curl -si -c "$JAR" -H 'X-Requested-With: XMLHttpRequest' -H 'Content-Type: application/json' \
  -d '{"username":"<name>","password":"<password>"}' "$BASE/api/v1/login"
curl -si -b "$JAR" "$BASE/api/v1/users/me"
#    200, JSON, Cache-Control: no-store. 401이면 Cookie가 오리진으로 전달되지 않는다.

# 5. X-Requested-With가 없는 쓰는 요청
curl -si -b "$JAR" -X POST "$BASE/api/v1/logout"
#    403, reason이 csrf. 헤더를 달았는데도 403이면 CDN이 헤더를 떼고 있거나(csrf) Origin을 바꾸고 있다(foreign_origin).

# 6. 파일의 Range 요청
curl -s -o /dev/null -D - -b "$JAR" \
  -H 'Range: bytes=0-99' "$BASE/api/v1/w/<workspace>/containers/<container>/files/<file>"
#    206, Content-Range: bytes 0-99/<전체 크기>, Cache-Control: private, no-cache

# 7. WebSocket 핸드셰이크. 101 뒤에는 연결이 열린 채로 있으므로 --max-time으로 끊는다.
curl -si --http1.1 --max-time 5 -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  -H "Origin: $BASE" -b "$JAR" "$BASE/ws"
#    HTTP/1.1 101 Switching Protocols. Origin을 다른 사이트로 바꾸면 403이어야 한다.
```

추가로 확인할 것:

- 웹 앱을 열어 둔 채 CDN의 유휴 타임아웃보다 오래 기다린 뒤, 다른 브라우저에서 보드를 고쳐 변경이 실시간으로 보이는지 본다.
- 한도에 가까운 크기의 파일을 UI에서 올려 본다. 실패하면 응답 헤더로 CDN이 답했는지 서버가 답했는지 구분한다.
- `POST /api/v1/login`을 임계값보다 많이 보내 rate limiting 규칙이 동작하는지 본다. CDN의 규칙보다 서버의 제한이 먼저 걸리면 429와 `Retry-After`가 서버에서 온다(응답에 `requestId`가 든 JSON 본문이 있다).
- 서버의 요청 로그에 찍히는 클라이언트 주소가 CDN의 주소가 아니라 실제 클라이언트의 주소인지 본다. CDN의 주소면 `trusted_proxies`가 비었거나 범위가 틀렸다.

## 6. 자주 생기는 문제

**배포 뒤에도 예전 `index.html`이 나온다.** `curl -sI`로 `Cache-Control: no-cache`가 그대로 오는지, 캐시 적중 표시(`X-Cache`, `CF-Cache-Status`, `Age`)가 있는지 본다. CloudFront에서는 기본 동작의 cache policy에서 Minimum TTL이 0인지 확인한다. 0보다 크면 `no-cache`가 있어도 그 시간만큼 캐시한다. Cloudflare에서는 HTML을 캐시 대상으로 만들면서 Edge TTL을 고정한 Cache Rule이 있는지 확인한다. 정책을 고친 뒤 무효화(invalidation, purge)를 한 번 실행한다. 해시가 있는 정적 파일은 이름이 바뀌므로 무효화할 필요가 없다.

**WebSocket이 일정한 간격으로 끊긴다.** 간격이 약 10분이고 CloudFront를 쓰면 유휴 타임아웃이다(3.3). CDN과 서버 사이에 로드 밸런서나 프록시가 더 있으면 그 유휴 타임아웃도 확인하고, ping 간격을 경로에서 가장 짧은 유휴 타임아웃보다 짧게 둔다. 연결이 아예 열리지 않으면 `/ws` 동작이 `Sec-WebSocket-*` 헤더를 전달하는지, Cloudflare의 WebSockets 토글이 켜져 있는지 본다.

**업로드가 413으로 실패한다.** Cloudflare에서는 플랜의 업로드 한도보다 큰 `upload_part_bytes`(4.3)가 가장 흔한 원인이다. 그 다음으로 서버의 `max_file_bytes`, CDN과 서버 사이 프록시의 본문 한도를 확인한다. CloudFront에 AWS WAF의 Core rule set이 붙어 있으면 413이 아니라 403으로 실패한다(3.5). 큰 아카이브 가져오기가 504(CloudFront)나 524(Cloudflare)로 끝나면 크기가 아니라 타임아웃 문제다(3.4, 4.3).

**로그인한 뒤 다시 로그인 화면으로 돌아간다.** 순서대로 확인한다.

1. `server_url`이 공개 URL과 같은지, base path가 CDN의 경로 패턴과 같은지 본다. 서버가 `index.html`에 채워 주는 주소가 틀리면 웹 앱이 다른 주소로 API를 부른다.
2. 5장의 4번 요청이 JSON으로 오는지 본다. HTML이 오면 요청이 `/api/*` 동작이 아니라 기본 동작으로 가고 있거나, custom error response가 응답을 바꾸고 있다.
3. CloudFront에서 `/api/*` 동작에 origin request policy가 붙어 있는지 본다. 없으면 쿠키가 오리진으로 가지 않아 로그인은 성공하고 그 뒤의 모든 조회가 401이 된다.
4. 로그인 응답의 `Set-Cookie`가 브라우저까지 그대로 오는지 본다. `Domain`이 붙었거나 `Path`가 `/`가 아니면 브라우저가 `__Host-` 쿠키를 버린다. `server_url`이 `http`인데 공개 주소가 `https`면 서버가 접두사 없는 이름을 쓰고 `Secure`를 붙이지 않는다.
5. 로그인 자체가 403이면 `Origin`이 `server_url`과 같은지 본다(스킴, 호스트, 포트).
6. 공유 보드 링크만 실패하면 쿼리 문자열(`readToken`)이 전달되는지 본다.

## 7. 검증 범위

이 문서의 구성은 실제 CloudFront 배포나 Cloudflare zone을 통과시켜 처음부터 끝까지 시험한 것이 아니다. 작성자들이 확인한 것은 서버의 테스트가 다루는 헤더 동작뿐이다. CDN 쪽 내용은 2026-11-10 기준 벤더 문서를 옮긴 것이고, WAF와 rate limiting 규칙은 실행해 보지 않은 예시다. 벤더의 한도, 기본값, 정책 내용은 바뀐다. 적용하기 전에 위에 적은 출처에서 다시 확인하고, 5장의 점검을 실제 배포에서 실행해야 한다.
