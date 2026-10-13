# Remote libSQL TLS test fixtures

Test-only certificates for the `db::backend::remote_tls` handshake tests of
the vendored libsql connector (`vendor/libsql-0.9.30/src/tls.rs`). Nothing
outside those tests uses them. `server.key` is a throwaway test key; it
protects nothing and is not registered anywhere.

| File | Contents |
| --- | --- |
| `ca.pem` | Self-signed test CA. The trusted root of the handshake tests. |
| `other-ca.pem` | Unrelated self-signed CA. The only root of the untrusted-CA test. |
| `server.pem` | Leaf issued by `ca.pem`, SAN `IP:127.0.0.1`. |
| `wrong-name.pem` | Leaf issued by `ca.pem`, SAN `DNS:wrong-name.invalid`. |
| `server.key` | P-256 PKCS#8 key of both leaves. |

Every certificate is valid from 2026-01-01T00:00:00Z until
2126-01-01T00:00:00Z. `fixture_leaves_verify_now_and_in_2100` fails when
either leaf no longer verifies against `ca.pem` now or on
2100-01-01T00:00:00Z. The CA keys were discarded after signing, so a
replacement regenerates the whole set: run these commands with OpenSSL 3.4 or
later in an empty directory and copy the five remaining files here.

```sh
V="-not_before 20260101000000Z -not_after 21260101000000Z"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -noenc -keyout ca.key -out ca.pem $V -subj "/CN=FVOCI remote TLS test CA"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -noenc -keyout other-ca.key -out other-ca.pem $V -subj "/CN=FVOCI remote TLS untrusted CA"
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out server.key
openssl req -new -key server.key -subj "/CN=FVOCI remote TLS server" -out server.csr
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n' > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 2 $V -extfile server.ext -out server.pem
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:wrong-name.invalid\n' > wrong-name.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 3 $V -extfile wrong-name.ext -out wrong-name.pem
rm ca.key other-ca.key server.csr server.ext wrong-name.ext
```
