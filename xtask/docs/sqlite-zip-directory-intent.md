# SQLite 중앙 ZIP 디렉터리 leaf의 의도

작성 기준은 지시 `6076625664` §2와 읽기 전용 원본
`1b977c16465c72b2095b3ba44d2aa1d545c4825c`의
`scripts/prepare-sqlite-build.sh:211-221`이다. 구현 parent는 별개인
`afba87caa8087eace087d5754ec23c9e2c21597d`다. 이 표를 Rust 소스보다 먼저 작성했다.

이 leaf는 입력 바이트에서 중앙 항목의 순서·중복·raw 이름·크기·외부 속성·local header offset·압축 방식·압축 크기·version needed를 읽는다.
최종 다섯 항목 정책, 이름 해석, 압축 해제, CRC, SQLite 빌드와 CLI 연결은 후속 B의 책임이다.
원본의 archive SHA-256 선검사와 기존 fixture는 변경하지 않는다.

| 원래 동작 | 새 동작 | 이유와 차이 |
| --- | --- | --- |
| `:211,214`의 `ZipFile.infolist()`는 중앙 항목을 순서대로 보유한다. | `Vec`에 모든 항목을 같은 순서로 반환한다. 중복 제거는 하지 않는다. | 원본 `:215`의 개수와 이름 집합 검사를 후속 호출자가 수행할 수 있게 한다. |
| `:215`는 정확히 다섯 항목과 고정 이름 집합을 요구한다. | parser 안에는 허용 이름·다섯 개 정책을 넣지 않는다. | 구조 파싱과 SQLite 정책을 분리한다. parser 성공을 archive 정책 수락으로 해석하면 안 된다. |
| `:218`은 `file_size`와 `external_attr >> 16`으로 크기·symlink를 거부한다. | ZIP64를 반영한 `u64` 크기와 원래 `u32` 외부 속성을 반환한다. | 상위의 16 MiB 제한과 동일한 symlink 판정에 필요한 정보를 보존한다. parser가 타입 정책을 추가하지 않는다. |
| `zipfile`은 ZIP64 EOCD·locator와, 한 항목 extra의 ZIP64 블록에서 크기·offset을 읽는다. 같은 extra에 ZIP64 블록이 또 있으면, 앞 블록이 남긴 값이 여전히 `0xFFFFFFFF` 또는 `u64::MAX`일 때만 다음 블록을 읽고, 그때 길이가 부족하면 거부한다. 값이 이미 풀렸거나 sentinel이 아니면 둘째 블록을 통과시킨다. CPython 3.13.5와 3.14.4가 같다. | 첫 ZIP64 extended-information(id `0x0001`)은 읽고, 같은 항목 extra에서 둘째 ZIP64 블록이 나오면 그 자리에서 거부한다. | added restriction. G3·G4는 이 제한 때문에 거부한다. hand `39`도 같은 이유로 거부한다. `38`·`38b`·`38d`는 Python zipfile도 거부한다. |
| 일반 EOCD의 disk 번호는 `ZipFile._RealGetContents`에서 별도로 거부하지 않는다. ZIP64 locator는 disk 번호가 0이 아니거나 총 disk 수가 1보다 크면 거부한다. | 일반 EOCD disk metadata를 반환하고, ZIP64 locator에는 같은 거부 조건을 적용한다. | 일반 disk label만으로 새 거부 정책을 만들지 않는다. 분할 파일 재조립이나 다중 디스크 지원을 주장하지 않는다. |
| `zipfile`은 중앙 크기와 footer 위치로 앞에 붙은 데이터 길이를 추론한다. EOCD offset 자체가 잘못되어도 일부 입력을 읽을 수 있다. | 같은 앞붙임 위치 계산을 사용하되 선언 offset·크기 합이 footer를 넘으면 오류다. | offset·길이 범위 초과를 명시 오류로 내라는 최신 지시의 강화다. |
| `zipfile`은 footer 뒤 바이트가 있는 경우에도 마지막 65,557바이트에서 EOCD를 찾을 수 있다. | 같은 검색 범위와 마지막 signature 선택 및 무주석 footer 우선 처리를 유지한다. 선언 comment 범위도 검사한다. | 후행 바이트 전면 금지 정책을 추가하지 않는다. 잘린 선언 comment는 범위 오류로 거부한다. |
| 중앙 고정 header·signature·extra 오류는 `zipfile` 오류다. 선언 길이가 남은 extra보다 큰 field는 Python zipfile이 거부한다. 일부 가변 필드의 짧은 read나 extra의 1~3바이트 꼬리는 통과할 수 있다. EOCD count는 순회 개수와 비교하지 않는다. CPython 3.13.5와 3.14.4에서 이 구분이 같다. | 고정·가변 필드와 extra envelope 전체를 검사하고 실제 중앙 항목 수를 EOCD 전체 count와 비교한다. 하나라도 깨지면 목록을 반환하지 않는다. | 최신 지시의 잘림·잘못된 signature·count·범위 오류 거부다. disk별 count는 별도 raw metadata로 보존한다. Python zipfile rejects a corrupt extra field whose declared size exceeds the bytes that remain. |
| version needed to extract의 하위 바이트가 63보다 크면 Python zipfile은 `NotImplementedError`로 거부한다. offset 6의 하위 바이트만 비교하고 offset 7은 reserved다. `0x0314`(G1)와 63(G2)은 통과한다. CPython 3.13.5와 3.14.4. | 같은 하위 바이트 비교로 거부한다. 반환하는 `version_needed`는 offset 6의 16비트 전체다. | Python과 같은 거부다. hand `42`·`44`·`69`와 D-B1은 거부한다. |
| 비압축 크기·압축 크기·local header offset이 ZIP64 marker `0xFFFFFFFF`(`0xFFFF`는 16비트 sentinel 표기)이고 대응 ZIP64 extra가 없으면, Python zipfile은 marker를 남긴 채 `infolist`를 성공시킨다. CPython 3.13.5와 3.14.4. D-N3, hand `34`. | 그 marker가 있는데 ZIP64 extra가 값을 주지 않으면 거부한다. | 새 parser만 거부한다. 더 엄격하고 안전하다. |
| `ZipInfo.filename`은 UTF-8/CP437 해독, NUL 처리, 조건부 Unicode Path extra 해석을 거친다. UTF-8 flag의 잘못된 UTF-8 이름과 깨진 Unicode Path extra(`0x7075`)는 Python zipfile이 거부한다. CPython 3.13.5와 3.14.4에서 그 입력을 거부한다. | raw 이름과 flags·extra를 그대로 반환한다. D-N1 세 파일(`D-N1-utf8-flag-invalid-name`, `D-N1-unicode-path-short`, `D-N1-unicode-path-bad-utf8`)과 hand `45`·`46`·`47`·`48`의 현재 결과는 수락이다. `#[ignore]`로 빼지 않는다. | 후속 B 커밋에서 그 입력을 거부한다. 이번 범위 밖이며, 이 leaf의 단위 성공을 이름 해석 parity로 세지 않는다. |
| `:220-221`은 두 파일을 읽으며 local header·압축·CRC 검사도 일어난다. | local header 본문·파일 데이터는 읽지 않는다. 각 항목에 `local_header_offset`(입력 기준 절대 offset), `compression_method`, `compressed_size`, `version_needed`를 반환하고, 선언된 local offset의 입력 범위만 검사한다. | 후속 read 교체에 필요한 중앙 값이다. local 대응·압축 해제·CRC·내용 hash·빌드·CLI 검증은 미구현이다. |

형식 대조는 [PKWARE APPNOTE](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)
§4.3.12, §4.3.14-16, §4.4.1.4, §4.5.3, 부록 D와
[CPython 3.13.5 zipfile 원문](https://github.com/python/cpython/blob/v3.13.5/Lib/zipfile/__init__.py)과
[CPython 3.14.4 zipfile 원문](https://github.com/python/cpython/blob/v3.14.4/Lib/zipfile/__init__.py)의
`_EndRecData`, `_EndRecData64`, `_handle_prepended_data`, `_RealGetContents`,
`ZipInfo._decodeExtra`, `_sanitize_filename`을 사용했다.
공식 문서의 다중 디스크 미지원 설명과 일반 EOCD label을 무시하는 실제 파싱을 구분한다.

코디네이터 추가 조건: 순수 std 중앙 parser와 이 의도 문서만 작성하고 A3와 소유권을 분리한다.
`main.rs`는 이 모듈을 등록해 CI가 비테스트 바이너리에서도 컴파일한다. `allow(dead_code)`는 쓰지 않는다. CLI 명령 연결은 후속이다.
