# smtp-sink.ts 와 smtp-sink.py

호출자가 보는 계약은 같다. `--capture`와 `--port-file`로 127.0.0.1의 평문 SMTP를 띄우고, DATA 한 통이 캡처 파일에 한 줄로 붙은 뒤에만 `250`을 보낸다. 아래는 그 계약을 맞추면서 Python `email` + `codecs`와 달라지는 지점이다.

| 항목 | 의도 | 이전 | 지금 | 남은 차이 / 이유 |
| --- | --- | --- | --- | --- |
| 본문 charset | Python `get_content`와 같은 문자열일 때만 `250`과 캡처를 남긴다 | `new TextDecoder(label)`에 라벨을 그대로 넘겼다. `x-user-defined`, `x-cp1252`, `x-mac-roman`, `unicode-1-1-utf-8`, `unicode11utf8`, `x-unicode20utf8`도 디코드되고 `250`이 나갔다 | 허용 목록만 디코드한다. ASCII와 ISO-8859-1은 직접 디코드한다. cp125x의 미정의 바이트는 U+FFFD로 고친다. 목록 밖은 기록 없이 연결을 끊고 `250`을 보내지 않는다 | `euc-kr`, `iso-2022-jp`, `gb2312`, `gbk`, `gb18030`, `big5`, `shift_jis`, `euc-jp`와 그 별칭은 Python이 디코드해 `250`을 보낸다. TextDecoder의 WHATWG 표가 Python `codecs`와 달라서 TS는 거부한다 |
| ISO-8859-1, US-ASCII, charset 생략 | 바이트 `0x80`의 문자가 Python과 같다 | TextDecoder가 둘 다 windows-1252로 넘겨 `0x80`이 U+20AC가 됐다. 비교 입력은 `[0xe9]`만 남았다 | ISO-8859-1은 바이트를 U+0000–U+00FF로 둔다 (`0x80` → U+0080). US-ASCII와 charset이 없는 본문은 `0x80`–`0xFF`를 U+FFFD로 둔다. 입력은 `[0x80, 0xe9]`다 | 이 세 경로는 Python과 같다 |
| windows-1252 | `0x80`은 U+20AC, 미정의 바이트는 U+FFFD | TextDecoder는 `0x81` 등을 C1 제어 문자로 둔다 | `0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`만 U+FFFD로 바꾼다 | 이 보정 뒤 0x00–0xFF는 Python cp1252와 같다 |
| RFC 2231 `charset*` | `charset*=utf-8''…`의 값이 본문 charset이다 | `charset=`만 봐서 `charset*=utf-8''not-a-charset`가 US-ASCII로 떨어지고 `250`이 나갔다 | `attribute*section*`와 percent-encoding을 풀고, 풀린 값이 허용 목록 밖이면 `250`을 보내지 않는다 | `charset*=utf-8''`처럼 값이 빈 확장 파라미터는 Python이 ASCII로 두고 TS는 알 수 없는 charset으로 거부한다 |
| RFC 2047 제목 | 제목 디코드 실패가 본문 실패와 같지 않다 | 알 수 없는 제목 charset을 UTF-8로 다시 디코드했다 | 허용 목록 밖 제목은 ASCII가 아닌 바이트마다 U+FFFD로 두고, 본문 charset이 허용되면 `250`을 보낸다 | Python은 `euc-kr` 같은 제목을 실제로 디코드한다. lookup 자체가 실패할 때만 바이트마다 U+FFFD다. TS는 허용 목록 밖 제목을 모두 후자로 둔다 |
| 테스트 프로세스 | `proc.kill()`이 sink를 끝내고 stdout을 닫는다 | 테스트를 `bun`으로 띄웠다. PATH의 `bun`이 자식으로 진짜 bun을 띄우면 kill이 래퍼만 죽이고 자식이 stdout을 붙잡는다 | TypeScript sink는 `process.execPath`로 띄운다. lockstep은 다시 `close`를 기다린다 | `6c17f0ed`의 QUIT/FIN 설명은 재현되지 않았다. hang의 원인은 래퍼가 남긴 자식이다 |
| 봉투 주소 | 캡처의 `from`/`to` | Python은 콜론 뒤를 `strip("<>")` 한다 | 첫 `<...>` 안쪽을 쓴다 | `MAIL FROM:<a@b.com> (주석)`에서 결과가 다르다. 테스트 호출자는 `<addr-spec>`만 보낸다 |
