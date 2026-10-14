import { expect, test } from "bun:test";
import {
  digest,
  evaluate,
  parseInput,
  type WitnessInput,
  type WitnessView,
} from "./process-witness.ts";

const environment = {
  FVOCI_DATABASE_BACKEND: "postgres",
  DATABASE_APP_URL: "synthetic-private-app-input",
  OIDC_GENERIC_CLIENT_SECRET: "synthetic-private-client-input",
};
const input = (): WitnessInput => ({
  pid: 200,
  parentPid: 100,
  startTicks: "2000",
  parentStartTicks: "1000",
  executableSha256: "a".repeat(64),
  environmentDigests: Object.fromEntries(
    Object.entries(environment).map(([key, value]) => [key, digest(value)]),
  ),
});
const view = (): WitnessView => ({
  before: { pid: 200, parentPid: 100, startTicks: "2000" },
  after: { pid: 200, parentPid: 100, startTicks: "2000" },
  parentBefore: { pid: 100, parentPid: 50, startTicks: "1000" },
  parentAfter: { pid: 100, parentPid: 50, startTicks: "1000" },
  callerBound: true,
  uids: [1000, 1000, 1000, 1000],
  gids: [1000, 1000, 1000, 1000],
  executableName: "fvoci-server",
  executableSha256: "a".repeat(64),
  executableAfterName: "fvoci-server",
  executableAfterSha256: "a".repeat(64),
  environment: { ...environment },
});

test("strict standalone DTO accepts digests and refuses arbitrary paths, commands and environment values", () => {
  expect(parseInput(input())).toEqual(input());
  for (const value of [
    null,
    [],
    { ...input(), path: "/arbitrary" },
    { ...input(), command: ["anything"] },
    { ...input(), pid: 0 },
    { ...input(), executableSha256: "wrong" },
    { ...input(), environmentDigests: { ...input().environmentDigests, EXTRA: digest("value") } },
    { ...input(), environmentDigests: environment },
  ])
    expect(() => parseInput(value)).toThrow();
});
test("witness facts require stable owning identities, the real actor/binary and complete matching app input", () => {
  expect(evaluate(input(), view()).accepted).toBe(true);
  for (const mutate of [
    (v: WitnessView) => {
      v.callerBound = false;
    },
    (v: WitnessView) => {
      v.after.startTicks = "2001";
    },
    (v: WitnessView) => {
      v.parentAfter.startTicks = "1001";
    },
    (v: WitnessView) => {
      v.before.parentPid = 101;
    },
    (v: WitnessView) => {
      v.uids[1] = 0;
    },
    (v: WitnessView) => {
      v.gids = [1000];
    },
    (v: WitnessView) => {
      v.executableName = "fvoci-migrate";
    },
    (v: WitnessView) => {
      v.executableSha256 = "b".repeat(64);
    },
    (v: WitnessView) => {
      v.executableAfterSha256 = "b".repeat(64);
    },
    (v: WitnessView) => {
      delete v.environment.DATABASE_APP_URL;
    },
    (v: WitnessView) => {
      v.environment.OIDC_GENERIC_CLIENT_SECRET = "wrong";
    },
    ...[
      "DATABASE_URL",
      "POSTGRES_PASSWORD",
      "TEST_DATABASE_URL",
      "KC_BOOTSTRAP_ADMIN_PASSWORD",
      "KC_E2E_PASSWORD_ALICE",
      "FVOCI_KC_OBSERVER_ENV",
    ].map((key) => (v: WitnessView) => {
      v.environment[key] = "synthetic-forbidden";
    }),
  ]) {
    const actual = view();
    mutate(actual);
    expect(evaluate(input(), actual).accepted).toBe(false);
  }
  const output = JSON.stringify(evaluate(input(), view()));
  for (const value of Object.values(environment)) expect(output).not.toContain(value);
});
