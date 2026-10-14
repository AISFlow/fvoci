// Local evidence only. The hosted guard and target allowlist remain separate.
import { closeSync, lstatSync, mkdirSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { localTursoUiAllocation } from "../selected-backend-ci/admission.ts";
import { digest, sha, write } from "../selected-backend-ci/io.ts";
import { ownedListener, startLocalLibsql } from "../keycloak/local-libsql.ts";
import {
  diagnosticJson,
  failureCode,
  get,
  require,
  root,
  token,
  UiError,
  valueDigest,
} from "./ui-common.ts";
import { lease } from "./ui-container.ts";
import { fixture, start, stop } from "./ui-native.ts";
import { identity, withProcesses } from "./ui-processes.ts";
import { currentBuild } from "./ui-record.ts";
import { executeRestore, sourceEnvironment } from "./ui-restore.ts";

export async function localRestore(): Promise<void> {
  const grant = localTursoUiAllocation("server");
  const manifest = currentBuild(lease.load);
  const selected = grant.localLibsql;
  require(selected !== undefined, "UI_LOCAL_ALLOCATION_REFUSED");
  mkdirSync(selected.directory, { recursive: true, mode: 0o700 });
  require(realpathSync(selected.directory) === selected.directory &&
    lstatSync(selected.directory).isDirectory() &&
    (lstatSync(selected.directory).mode & 0o777) === 0o700, "RESTORE_LOCAL_ROOT_REFUSED");
  await withProcesses(async (scope) => {
    const before = scope.allocations.length;
    const directory = join(selected.directory, selected.label + "-" + token(6));
    const local = await startLocalLibsql(scope, selected.binary.path, directory);
    let original: unknown = null;
    try {
      const allocations = scope.allocations.slice(before);
      require(allocations.filter((a) => a.label === "official-sqld").length ===
        1, "RESTORE_LOCAL_SOURCE_REFUSED");
      const allocation = scope.allocations.findIndex(
        (a, index) => index >= before && a.label === "official-sqld",
      );
      const child = scope.allocations[allocation]?.process;
      require(child !== undefined, "RESTORE_LOCAL_SOURCE_REFUSED");
      const upstream = await ownedListener(child);
      const url = new URL(local.environment.FVOCI_LIBSQL_URL as string);
      const ca = local.environment.SSL_CERT_FILE as string;
      require(url.protocol === "https:" &&
        url.hostname === "127.0.0.1" &&
        url.port !== "" &&
        url.port !== "0" &&
        url.pathname === "/" &&
        url.search === "" &&
        url.hash === "" &&
        url.username === "" &&
        url.password === "", "RESTORE_LOCAL_TARGET_REFUSED");
      require(ca === join(directory, "tls.crt") &&
        realpathSync(ca) === ca &&
        (lstatSync(ca).mode & 0o777) === 0o600 &&
        sha(ca) === local.certificateSha256, "RESTORE_LOCAL_CA_REFUSED");
      // The module's official-pinned child and observed own socket precede
      // publication of the private input and any native/browser consumption.
      write(join(root(), "local-source-binding.private.json"), {
        source: grant.source,
        tree: grant.tree,
        inputSha256: grant.inputSha256,
        manifestSha256: sha(join(root(), "current-build.json")),
        physicalSha256: manifest.physicalInputs.sha256,
        child: identity(child.pid),
        targetSha256: digest(url.origin),
        upstreamSha256: digest(upstream),
        certificateSha256: local.certificateSha256,
        caSha256: sha(ca),
        sqldSha256: sha(selected.binary.path),
        loopbackOnly: true,
        certificateSan: "IP-loopback",
        browserCredentials: false,
      });
      const environment: Record<string, string> = {
        ...local.environment,
        FVOCI_DATABASE_BACKEND: "libsql-remote",
        FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
        FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
        PASSWORD_PEPPER_KEYS: diagnosticJson({ fixture: token(32) }),
        PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
        ENCRYPTION_KEYS: diagnosticJson({ "turso-test": token(32) }),
        ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
      };
      const bootstrap = join(root(), "local-bootstrap");
      mkdirSync(bootstrap, 0o700);
      const storage = join(bootstrap, "storage");
      mkdirSync(storage, 0o700);
      const started = await start(
        scope,
        manifest,
        sourceEnvironment(manifest, environment, storage, "tui-" + token(10)),
        bootstrap,
        true,
      );
      try {
        await stop(scope, started.server, started.base, bootstrap);
      } finally {
        closeSync(started.log);
      }
      // The local fixture runs in the same container shape and needs a storage mount.
      const baseline = await fixture(scope, manifest, "baseline", {
        ...environment,
        FVOCI_STORAGE_DIR: storage,
      });
      write(join(root(), "local-baseline.private.json"), {
        baseline,
        source: grant.source,
        tree: grant.tree,
        baselineSha256: valueDigest(baseline),
        targetSha256: digest(environment.FVOCI_LIBSQL_URL as string),
      });
      const result = await executeRestore(scope, manifest, baseline, environment, undefined, {
        heavy: grant.heavy.path,
        sourceAllocation: allocation,
      });
      require(result.result === "PASS" &&
        get(result, "preservation", "result") === "PASS", "RESTORE_LOCAL_ACCEPTANCE_FAILED");
    } catch (error) {
      original = error;
    }
    try {
      await local.close();
    } catch {
      if (original === null) original = new UiError("RESTORE_LOCAL_SOURCE_CLOSURE_FAILED");
      else
        scope.io.output.err(
          diagnosticJson({
            originalFailure: failureCode(original),
            cleanupErrors: ["RESTORE_LOCAL_SOURCE_CLOSURE_FAILED"],
          }),
        );
    }
    if (original !== null) throw original as Error;
  });
}
