// The CLI as the workflow and the browser fixture run it: a real Bun child,
// real owned grandchildren, real files. No credentials or network.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawn } from "bun";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { sha, write } from "../selected-backend-ci/io.ts";
import { must } from "./ui-fakes.ts";
import { memberWrapper } from "./ui-flow.ts";

const entry = join(import.meta.dir, "ui.ts");
let directory: string;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-cli-"));
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
});

async function cli(args: string[], env: Record<string, string>) {
  const child = spawn([process.execPath, entry, ...args], {
    env: { PATH: process.env.PATH ?? "", ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { stdout, stderr, code };
}

describe("CLI contract", () => {
  test("modes other than the three explicit ones are refused with 78", async () => {
    for (const args of [[], ["--consume"], ["--freeze", "extra"], ["record-before"]])
      expect(await cli(args, {})).toEqual({
        stdout: "",
        stderr: "UI_EXPLICIT_MODE_REQUIRED\n",
        code: 78,
      });
  });

  test("recording refuses outside its hosted allocation before writing", async () => {
    expect(await cli(["--record-before"], {})).toEqual({
      stdout: "",
      stderr: "UI_CONSUMER_FAILED\n",
      code: 78,
    });
    const runner = { RUNNER_TEMP: directory };
    expect(await cli(["--record-before"], runner)).toMatchObject({
      stderr: "UI_HOSTED_ALLOCATION_REQUIRED\n",
      code: 78,
    });
    expect(
      await cli(["--record-before"], {
        ...runner,
        GITHUB_ACTIONS: "true",
        CI: "true",
        GITHUB_JOB: "turso-ui",
        GITHUB_RUN_ID: "1",
      }),
    ).toMatchObject({ stderr: "UI_HOSTED_ALLOCATION_REQUIRED\n", code: 78 });
    expect(
      await cli(["--record-before"], { ...runner, FVOCI_SELECTED_EXECUTION_MODE: "other" }),
    ).toMatchObject({
      stderr: "UI_EXECUTION_MODE_REFUSED\n",
      code: 78,
    });
    expect(
      await cli(["--freeze"], { ...runner, FVOCI_SELECTED_EXECUTION_MODE: "orca-local" }),
    ).toMatchObject({
      stderr: "UI_CONSUMER_FAILED\n",
      code: 78,
    });
    expect(readdirSync(join(directory, "turso-ui"))).toEqual([]);
  });
});

describe.skipIf(process.platform !== "linux")("member actor", () => {
  const namespace = "tui-" + "0123456789abcdef0123";
  const member = {
    namespace,
    userId: "cccccccc-cccc-cccc-cccc-cccccccccccc",
    workspaceId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
    email: namespace + "-member@example.invalid",
    commit: "confirmed",
    freshPrimaryReadback: true,
    lifecycleDrain: "confirmed",
    leases: 0,
    serverCloseReceipt: "not-exposed-by-sdk",
  };
  function actorInputs(script: string) {
    const fixture = join(directory, "fvoci-e2e-fixture");
    writeFileSync(fixture, "#!/bin/sh\n" + script);
    chmodSync(fixture, 0o700);
    const run = join(directory, "on");
    mkdirSync(run, 0o700);
    const capsule = join(run, "actor-input.private.json");
    write(capsule, {
      manifest: { binaries: { "fvoci-e2e-fixture": { path: fixture, sha256: sha(fixture) } } },
      environment: {
        FVOCI_LIBSQL_URL: "libsql://invented.invalid",
        FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN",
      },
      namespace,
      workspaceId: member.workspaceId,
    });
    return {
      capsule,
      run,
      env: {
        RUNNER_TEMP: directory,
        E2E_DATABASE_BACKEND: "libsql-remote",
        FVOCI_E2E_TURSO_PRIVATE_INPUT: capsule,
        FVOCI_E2E_TURSO_NAMESPACE: namespace,
        E2E_USER_EMAIL: namespace + "-member@example.invalid",
        E2E_USER_PASSWORD: "memberpass1",
        E2E_USER_GIVEN_NAME: "협업",
        E2E_USER_FAMILY_NAME: "멤버",
        E2E_WORKSPACE_SLUG: namespace,
        E2E_MEMBERSHIP_ROLE: "member",
      },
    };
  }

  test("creates the member through the native fixture and records the receipt", async () => {
    const inputs = actorInputs("cat >/dev/null\nprintf '%s' '" + JSON.stringify(member) + "'\n");
    const result = await cli(["--actor"], inputs.env);
    expect(result).toEqual({ stdout: member.userId + "\n", stderr: "", code: 0 });
    expect(
      JSON.parse(readFileSync(join(inputs.run, "member-" + member.userId + ".json"), "utf8")),
    ).toEqual(member);
    const closure = readdirSync(join(directory, "turso-ui")).filter((n) =>
      n.startsWith("process-closure-"),
    );
    expect(closure).toHaveLength(1);
    expect(
      JSON.parse(readFileSync(join(directory, "turso-ui", must(closure[0])), "utf8")),
    ).toMatchObject({
      confirmed: true,
      normalClosure: true,
    });
  });

  test("the member wrapper runs the actor with the browser fixture's environment", async () => {
    // selected-backend-fixture.ts passes PATH, LANG, the E2E actor values, the
    // private input and the namespace; RUNNER_TEMP is not among them.
    const inputs = actorInputs("cat >/dev/null\nprintf '%s' '" + JSON.stringify(member) + "'\n");
    const fixtureEnv: Record<string, string> = { ...inputs.env, PATH: process.env.PATH ?? "" };
    Reflect.deleteProperty(fixtureEnv, "RUNNER_TEMP");
    const wrapper = join(inputs.run, "member-fixture");
    writeFileSync(wrapper, memberWrapper(directory), { mode: 0o700 });
    const child = spawn([wrapper], { env: fixtureEnv, stdout: "pipe", stderr: "pipe" });
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);
    expect({ stdout, stderr, code }).toEqual({ stdout: member.userId + "\n", stderr: "", code: 0 });
    expect(
      readdirSync(join(directory, "turso-ui")).some((n) => n.startsWith("process-closure-")),
    ).toBe(true);
  });

  test("refuses changed inputs and a failing or lingering native fixture", async () => {
    const ok = "cat >/dev/null\nprintf '%s' '" + JSON.stringify(member) + "'\n";
    let inputs = actorInputs(ok);
    expect(await cli(["--actor"], { ...inputs.env, E2E_MEMBERSHIP_ROLE: "owner" })).toMatchObject({
      stderr: "UI_ACTOR_INPUT_REFUSED\n",
      code: 78,
    });
    expect(
      await cli(["--actor"], { ...inputs.env, FVOCI_E2E_TURSO_NAMESPACE: "tui-other" }),
    ).toMatchObject({
      stderr: "UI_ACTOR_NAMESPACE_REFUSED\n",
      code: 78,
    });
    writeFileSync(join(directory, "fvoci-e2e-fixture"), "#!/bin/sh\nexit 0\n");
    expect(await cli(["--actor"], inputs.env)).toMatchObject({
      stderr: "UI_ACTOR_BINARY_CHANGED\n",
      code: 78,
    });
    rmSync(directory, { recursive: true, force: true });
    directory = mkdtempSync(join(tmpdir(), "fvoci-ui-cli-"));
    inputs = actorInputs(
      "cat >/dev/null\nprintf '%s' '{\"originalFailure\":\"TURSO_UI_ACTOR_FAILED\"}'\nexit 3\n",
    );
    const failed = await cli(["--actor"], inputs.env);
    expect(failed).toMatchObject({ stdout: "", stderr: "TURSO_UI_ACTOR_FAILED\n", code: 78 });
    expect(
      readdirSync(join(directory, "turso-ui")).some((n) => n.startsWith("fixture-failure-")),
    ).toBe(true);
    rmSync(directory, { recursive: true, force: true });
    directory = mkdtempSync(join(tmpdir(), "fvoci-ui-cli-"));
    // A detached grandchild that outlives the helper is killed and the closure is refused.
    inputs = actorInputs(
      "cat >/dev/null\nsetsid sleep 60 >/dev/null 2>&1 &\nprintf '%s' '" +
        JSON.stringify(member) +
        "'\n",
    );
    const lingering = await cli(["--actor"], inputs.env);
    expect(lingering.code).toBe(78);
    expect(lingering.stderr.trim().split("\n").pop()).toBe("UI_PROCESS_CLOSURE_FAILED");
    expect(lingering.stderr).not.toContain("PRIVATE_TOKEN");
    expect(existsSync(join(inputs.run, "member-" + member.userId + ".json"))).toBe(false);
  }, 60000);
});
