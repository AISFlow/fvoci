# smtp-sink.ts 와 smtp-sink.py

호출자는 단순한 메시지만 보낸다. 지원하는 헤더 문법만 파싱하고, 그 밖은 캡처하지 않으며 `250`을 보내지 않는다. Python보다 엄격하게 거부하는 것은 허용된다. Accepted messages are stored the same as Python except for the differences listed below.

## Supported input grammar

```
WSP        = SP / HTAB
token      = 1*(%x21-7E except tspecials "(" / ")" / "<" / ">" / "@" / "," / ";" / ":" / "\" / DQUOTE / "/" / "[" / "]" / "?" / "=")
attr       = token without "*"
media-type = [WSP] token "/" token *( [WSP] ";" [WSP] parameter )
parameter  = attr [WSP] ["*" 1*DIGIT] ["*"] "=" [WSP] (token / DQUOTE 1*token DQUOTE)
CTE        = [WSP] ("7bit" / "8bit" / "binary" / "base64" / "quoted-printable") [WSP]
```

`*`와 `=` 사이에는 WSP가 없다. 헤더 이름은 콜론 바로 앞까지 인쇄 가능한 ASCII다. 이름 앞의 U+FEFF와 콜론 앞의 공백(`Content-Type :`)은 문법 밖이다. 주석 `(...)`, NBSP를 포함한 비ASCII, VT(`\x0b`), FF(`\x0c`), NEL(U+0085)도 문법 밖이다. charset 라벨은 ASCII 제어 문자와 공백이 없는 token이다. `utf 8`처럼 가운데 공백이 있으면 거부한다. boundary 값도 공백·주석·NBSP가 없는 token이다. base64 본문은 SP/HTAB/CR/LF를 뺀 뒤 알파벳만 허용하고, `=`는 끝에만 두며 길이는 4의 배수다. 제목의 encoded-word charset도 본문과 같은 허용 목록이다.

| 항목 | 의도 | 이전 | 지금 | 남은 차이 / 이유 |
| --- | --- | --- | --- | --- |
| 주석·NBSP·빈 파라미터 | Content-Type, Content-Disposition, Content-Transfer-Encoding이 문법 밖이면 거부한다 | `charset (c)=bad`, `(c) charset=`, `charset\u00a0=`를 건너뛰거나 US-ASCII로 두고 `250`을 보낼 수 있었다 | 주석, NBSP, 빈 파라미터는 기록도 `250`도 없다 | TS가 더 엄격하다. Python은 일부를 고쳐 받아들인다 |
| 타입 토큰 | `type/subtype`만 미디어 타입이다 | `Content-Type: text;`를 text/plain으로 넘길 수 있었다 | `text;`처럼 서브타입이 없거나 토큰이 비면 거부한다 | TS가 더 엄격하다 |
| boundary | 구분자는 공백·주석·NBSP가 없는 token이다 | `b `, `b(c)`, `boundary (c)=b`를 다른 boundary로 읽을 수 있었다 | 그 세 형태는 거부한다 | 받아 두면 Python이 자른 본문과 달라질 수 있다 |
| CTE | 전송 인코딩은 토큰 하나다 | `base64 (c)`, `base64; x=y`, `base64\x85`, `quoted-printable (c)`를 base64로 풀 수 있었다 | 주석, 파라미터, 비ASCII, 뒤쪽 쓰레기가 있으면 거부한다 | Python은 그 값을 base64로 보지 않고 원문을 둔다. TS는 다른 본문을 저장하지 않도록 거부한다 |
| `\x0b` `\x0c` charset | 제어 문자를 라벨에서 잘라 내지 않는다 | 부모 `84abf9a5`의 `trim()`은 VT·FF를 지워 `\x0butf-8`과 `\x0cutf-8`을 utf-8로 승인했다 | 둘 다 문법 밖이라 거부한다 | Python은 이 라벨로 `A`를 디코드해 `250`을 보낸다. TS는 거부한다 |
| charset 라벨 | ASCII `A`–`Z`만 소문자로 접고, 제어·공백·비ASCII는 거부한다 | `toLowerCase()`가 U+212A를 `k`로, U+0130을 `i`와 결합 문자로 바꿀 수 있었다 | `\u212aoi8-r`, `\u0130SO-8859-1`, `utf 8`, `utf\t8`은 거부한다 | `utf 8`은 가운데 공백이다. Python은 공백 앞 `utf`만 보고 받아들일 수 있다 |
| `charset* =` | `*` 바로 뒤가 `=`일 때만 확장 파라미터다 | `charset* =utf-8''…`를 버리고 US-ASCII로 `250`을 보냈다 | `*`와 `=` 사이의 공백은 거부한다. 이름과 `*` 사이의 WSP(`charset *=`)는 확장으로 읽는다 | TS가 더 엄격하다 |
| base64 `=` | `=` 뒤에 데이터가 있으면 다른 본문을 저장하지 않는다 | `aGk=x`와 `aGk*!!=x`를 앞부분만 풀면 Python과 달라질 수 있다. 길이가 4의 배수가 아닌 `aGkx1`도 달랐다 | 그 본문은 거부한다. 패딩이 끝에만 있는 `aGk=`는 `hi`다 | Python 3.12는 `aGk=x`와 `aGk*!!=x`를 `hi`로 저장한다. 등호가 없는 `aGkx`는 `hi1`이고, `aGkx1`은 원문 `aGkx1`이다. TS는 거부한다 |
| 본문 charset | 허용 목록이고 바이트가 보존될 때만 Python과 같은 문자열에 `250`을 보낸다 | TextDecoder에 라벨을 그대로 넘겼다 | ASCII와 ISO-8859-1은 직접 디코드한다. cp125x 미정의 바이트는 U+FFFD다. 목록 밖은 거부한다 | `euc-kr`, `iso-2022-jp`, `gb2312`, `gbk`, `gb18030`, `big5`, `shift_jis`, `euc-jp`는 Python이 `250`을 보낸다. WHATWG 표가 달라 TS는 거부한다 |
| ISO-8859-1, US-ASCII, charset 생략 | base64·quoted-printable로 바이트가 남을 때 `0x80`이 Python과 같다 | TextDecoder가 `0x80`을 U+20AC로 뒀다 | ISO-8859-1은 U+0080. US-ASCII와 charset 생략은 `0x80`–`0xFF`를 U+FFFD로 둔다. 입력은 `[0x80, 0xe9]` | 8bit 본문에는 적용하지 않는다 |
| BOM | utf-8·utf-16le·utf-16be는 U+FEFF를 남기고, utf-16은 BOM으로 엔디안만 고른다 | TextDecoder가 BOM을 지우거나 FE FF를 리틀엔디안으로 풀었다 | `ignoreBOM`으로 BOM을 남긴다. `utf-16`의 FE FF는 뒤를 utf-16be로 푼다 | 받아들인 BOM 본문은 Python과 같다 |
| windows-1252 | `0x80`은 U+20AC, 미정의 바이트는 U+FFFD | TextDecoder는 `0x81` 등을 C1로 둔다 | `0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`만 U+FFFD | 보정 뒤 0x00–0xFF는 Python cp1252와 같다 |
| RFC 2231 `charset*` | `charset*=utf-8''…`의 값이 본문 charset이다 | 확장 형태를 무시하면 US-ASCII로 `250`이 나갔다 | 문법 안의 `charset*`를 풀고, 값이 허용 목록 밖이면 거부한다 | 값이 빈 `charset*=utf-8''`는 Python이 ASCII로 두고 TS는 거부한다 |
| RFC 2047 제목 | 제목 charset도 허용 목록이다 | 허용 목록 밖 제목을 U+FFFD로 저장하고 `250`을 보냈다 | `=?euc-kr?b?x9GxuQ==?=`처럼 목록 밖이면 기록도 `250`도 없다 | Python은 `euc-kr` 제목을 디코드하고 `250`을 보낸다. TS는 거부한다 |
| 봉투 주소 | 캡처의 `from`/`to` | Python은 콜론 뒤를 `strip("<>")` 한다 | 첫 `<...>` 안쪽을 쓴다 | U+001F와 U+0085는 주소의 양쪽 끝에 그대로 남는다. JS `trim`은 이 둘을 공백으로 보지 않는다. U+FEFF는 `trim`이 제거한다 |
| 8bit 비ASCII 본문 | SMTP 줄을 UTF-8로 읽은 뒤 charset으로 다시 푼다 | Python은 U+00FF 이하를 한 바이트로 되돌리고, 그 위는 `raw-unicode-escape`로 둔다 | TS는 JS 문자열을 UTF-8 바이트로 다시 인코드한 뒤 charset으로 푼다 | charset이 utf-8이고 본문이 `한`(UTF-8 ED 95 9C, U+D55C)이면 Python 본문은 ASCII 여섯 자 `\ud55c`이고 TS 본문은 문자 `한`이다. base64로 바이트를 넘기면 이 차이가 없다 |
| `DATA\r\r`, `.\r\r` | CR을 하나만 걷는다 | Python `rstrip("\\r")`는 `DATA`와 `.`로 본다 | `DATA\r`와 `.\r`는 그 명령이 아니다 | TS가 더 엄격하다 |
| BOM 헤더 이름 | 헤더 이름의 U+FEFF는 거부한다 | TS `trim`이 BOM을 지워 헤더로 인정하면, charset이 utf-8이고 본문이 base64 `aGk=`일 때 TS는 `hi`로 저장한다. Python은 그 줄을 헤더로 보지 않아 `aGk=`를 본문에 그대로 두고 `250`을 보낸다 | BOM으로 시작하는 헤더 이름은 기록도 `250`도 없다 | Python은 여전히 `250`과 원문 `aGk=`를 남긴다. TS는 다른 본문을 저장하지 않도록 거부한다 |
| 콜론 앞 공백 | `Content-Type :`는 헤더가 아니다 | TS가 콜론 앞 공백을 지우고 Content-Type으로 보면, 그 다음 CTE base64를 풀어 `aGk=`가 `hi`가 된다. Python은 그 줄부터를 본문으로 보고 `aGk=`를 그대로 둔 채 `250`을 보낸다 | 콜론 앞에 공백이 있으면 기록도 `250`도 없다 | Python은 여전히 `250`과 원문 `aGk=`를 남긴다. TS는 거부한다 |
| 테스트 프로세스 | `proc.kill()`이 sink stdout을 닫는다 | PATH의 `bun` 래퍼를 죽이면 자식이 stdout을 붙잡는다 | TypeScript sink는 `process.execPath`로 띄운다. lockstep은 소켓이 아직 열려 있을 때만 `close`를 기다린다 | hang의 원인은 래퍼가 남긴 자식이다 |
