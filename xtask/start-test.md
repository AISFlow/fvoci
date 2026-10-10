# `start-test` contract and old/new differences

`cargo xtask start-test postgres|minio|meili [command...]` replaces the bodies of `scripts/start-test-postgres.sh`, `scripts/start-test-minio.sh`, and `scripts/start-test-meili.sh`. The shell files stay on their current paths and, in the following commit, only exec this subcommand.

No registry crate was added. `xtask` still depends only on the standard library, and the root `Cargo.lock` is unchanged.

## Contract

| Behavior | Contract |
| --- | --- |
| Services | `postgres`, `minio`, `meili` |
| Tools | PostgreSQL and Meilisearch require `docker` and `openssl`. MinIO also requires `curl`. The first missing tool exits 1 with `{tool} is required for local test {PostgreSQL\|MinIO\|Meilisearch}`. |
| Postgres major | `FVOCI_TEST_PG_MAJOR` unset selects 18. Empty or any other value exits 2: `FVOCI_TEST_PG_MAJOR must be 16, 17 or 18 (got '…')`. Images are the digests pinned in `.github/workflows/rust.yml`. |
| Postgres connections | `FVOCI_TEST_PG_MAX_CONNECTIONS` unset or empty selects `150`. |
| MinIO / Meilisearch command | No command exits 1 with a usage line. |
| Postgres command | No command runs `scripts/run-db-tests.sh` from the repository that contains this package. |
| Publish | `127.0.0.1:0` to container ports 5432, 9000, and 7700. The host port is the last `:` field of the first `docker port` line. |
| Names | `fvoci-rust-test-pg-`, `fvoci-rust-test-minio-`, and `fvoci-rust-test-meili-` plus the 32 hex run id. Label `fvoci.test-run` is that id. |
| Secrets | `openssl rand -hex`. Postgres and MinIO pass container credentials in a mode `0600` env file, not on `docker` argv. Meilisearch passes `MEILI_MASTER_KEY` with `-e`. |
| Child environment | Postgres: `TEST_DATABASE_URL`, `FVOCI_TEST_PG_CONTAINER`. MinIO: `S3_ENDPOINT`, `S3_REGION` (default `us-east-1`), `S3_BUCKET` (`fvoci-test-` + first 12 run-id hex digits), `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, `S3_FORCE_PATH_STYLE` (default `1`), `FVOCI_TEST_MINIO_CONTAINER`. Meilisearch: `FVOCI_MEILI_URL`, `FVOCI_MEILI_KEY`, `MEILI_MASTER_KEY`, `FVOCI_TEST_MEILI_CONTAINER`. |
| Readiness | 30s. Postgres uses `pg_isready` then `SHOW server_version_num` (`^{major}[0-9]{4}$`). MinIO uses `curl -fsS` on `/minio/health/ready`. Meilisearch uses `wget` of `/health` inside the container. |
| Timeout stderr | `postgres did not become ready within 30s`; MinIO adds the last 20 `docker logs` lines; Meilisearch adds the full logs. |
| Cleanup | `docker rm -f -v` the container name, then delete the env file, including after a failed command. |
| Status | The command status is the process status. A missing path exits 127. SIGINT exits 130 and SIGTERM exits 143, after the container is removed. |

## Comparison

One old-vs-new run against the same fake `docker`, `openssl`, and `curl` is in the review log, not in this repository. With deterministic `openssl` output, child environment, stdout, and normalized `docker` argv matched for Meilisearch success, PostgreSQL 16 with `max_connections=40`, PostgreSQL default major with an empty max-connections value, PostgreSQL version mismatch, MinIO defaults, and MinIO with region, path style, and CORS set. A real MinIO readiness timeout (25 log lines, about 30s each side) matched exit 1, the timeout line, and `line-6` through `line-25`. SIGTERM to the supervisor was exit 143 on both sides and both removed the container.

The first missing-`docker` attempt in that log is not evidence: `PATH` hid `bash` before the shell script ran. The corrected rerun matched all three missing-tool messages and exit code 1.

| Item | Shell | xtask | Observed |
| --- | --- | --- | --- |
| Usage program name | `$0`, for example `/workspace/scripts/start-test-minio.sh` | `cargo xtask start-test minio` (and the same shape for `meili`) | stderr differs; exit code stays 1 |
| Missing command prefix | `{script}: line 59: {command}: …` | `{command}: …` | exit 127 both. A path says `No such file or directory`; a bare name says `command not found` |
| Env-file name | `mktemp` suffix `fvoci-pg-env.XXXXXX` / `fvoci-minio-env.XXXXXX` | `fvoci-pg-env.{run id}` / `fvoci-minio-env.{run id}` | mode `0600`, body, and every other `docker` argument matched after the path was normalized |
| Readiness deadline | `sleep 1` can run past the 30s mark | stop at 30s | the 25-line MinIO timeout text still matched |
| Unused Meilisearch `ROOT` | assigned and unused | not assigned | no child, docker, or status difference |
| Interrupt | `trap` then `exit 130` / `exit 143` | handler forwards the signal to the command, then exits 130 / 143 | both SIGTERM runs exited 143 and removed the container; the command was not left running |

The default PostgreSQL command was not executed in the comparison, because that starts `scripts/run-db-tests.sh`. Both implementations select that script from the repository root.
