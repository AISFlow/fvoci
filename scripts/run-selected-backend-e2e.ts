#!/usr/bin/env bun
// Selected backend CLI called by scripts/run-web-e2e.sh.
import { strict as assert } from "node:assert";
import { realpathSync, statSync } from "node:fs";
import { isAbsolute } from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";
import { recordAfter, recordBefore, stage } from "../tools/selected-backend-ci/build.ts";
import { configList } from "../tools/selected-backend-ci/config-list.ts";
import { root, uid } from "../tools/selected-backend-ci/io.ts";
import { ownershipReturn, run, runtimePermissions } from "../tools/selected-backend-ci/runtime.ts";

export const modes = [
  "record-before",
  "stage",
  "record-after",
  "run",
  "permissions",
  "owner-return",
  "config-list",
] as const;
class CliError extends Error {}
export interface Arguments {
  mode: (typeof modes)[number];
  output: string;
  stageName?: string;
  sqliteParent?: string;
  dockerGid?: number;
  lane?: string;
  command: string[];
}
export function parseCLI(args: string[]): Arguments | null {
  const options = {
    output: { type: "string" },
    "stage-name": { type: "string" },
    "sqlite-parent": { type: "string" },
    "docker-gid": { type: "string" },
    lane: { type: "string" },
    help: { type: "boolean", short: "h" },
  } as const;
  // parseArgs owns option parsing. Tokens retain the exact stage argv, including
  // unknown compiler switches; no shell, quoting parser or runner delegation.
  try {
    const result = parseArgs({
      args,
      options,
      strict: false,
      allowPositionals: true,
      tokens: true,
    });
    if (result.values.help) return null;
    for (const name of ["output", "stage-name", "sqlite-parent", "docker-gid", "lane"] as const) {
      if (result.values[name] !== undefined && typeof result.values[name] !== "string")
        throw new CliError("missing option value");
    }
    const mode = result.positionals[0];
    if (!modes.includes(mode as (typeof modes)[number]) || typeof result.values.output !== "string")
      throw new CliError("mode and --output are required");
    const claimed = new Set<number>();
    let modeClaimed = false;
    for (const token of result.tokens) {
      if (token.kind === "positional" && !modeClaimed) {
        claimed.add(token.index);
        modeClaimed = true;
      } else if (token.kind === "option" && token.name in options) {
        claimed.add(token.index);
        if (token.value !== undefined && !token.inlineValue) claimed.add(token.index + 1);
      }
    }
    const command = args.filter((_, index) => !claimed.has(index));
    if (command[0] === "--") command.shift();
    const docker = result.values["docker-gid"];
    if (
      docker !== undefined &&
      (typeof docker !== "string" ||
        !/^[+-]?[0-9]+$/.test(docker) ||
        !Number.isSafeInteger(Number(docker)))
    )
      throw new CliError("invalid --docker-gid");
    return {
      mode: mode as (typeof modes)[number],
      output: result.values.output,
      stageName: result.values["stage-name"] as string | undefined,
      sqliteParent: result.values["sqlite-parent"] as string | undefined,
      dockerGid: docker === undefined ? undefined : Number(docker),
      lane: result.values.lane as string | undefined,
      command,
    };
  } catch (error) {
    if (error instanceof CliError) throw error;
    throw new CliError("invalid command arguments");
  }
}
export async function main(argv = process.argv.slice(2)): Promise<number> {
  const args = parseCLI(argv);
  if (!args) {
    process.stdout.write(
      "usage: run-selected-backend-e2e.ts {" +
        modes.join(",") +
        "} --output OUTPUT [--stage-name NAME] [--sqlite-parent PATH] [--docker-gid INTEGER] [--lane LANE/FLOW] [-- COMMAND ...]\n",
    );
    return 0;
  }
  assert.ok(
    args.mode === "stage" || !args.command.length,
    "unexpected arguments outside compiler stage",
  );
  assert.ok(
    args.lane === undefined || args.mode === "run" || args.mode === "owner-return",
    "lane argument is only for the selected runtime",
  );
  assert.ok(
    args.mode === "permissions" ||
      (args.sqliteParent === undefined && args.dockerGid === undefined),
    "unexpected runtime permission arguments",
  );
  if (args.mode === "config-list")
    assert.ok(isAbsolute(args.output) && realpathSync(args.output) === args.output);
  const output = realpathSync(args.output),
    facts = statSync(output);
  assert.ok(facts.isDirectory() && facts.uid === uid() && (facts.mode & 0o777) === 0o700);
  switch (args.mode) {
    case "record-before":
      recordBefore(output);
      break;
    case "record-after":
      recordAfter(output);
      break;
    case "stage":
      return stage(output, args.stageName, args.command);
    case "permissions":
      assert.ok(args.sqliteParent && args.dockerGid !== undefined);
      await runtimePermissions(output, args.sqliteParent, args.dockerGid);
      break;
    case "owner-return":
      ownershipReturn(output, undefined, args.lane);
      break;
    case "config-list":
      return configList(output);
    case "run":
      return await run(output, undefined, undefined, args.lane);
  }
  return 0;
}
if (import.meta.main) {
  try {
    process.exitCode = await main();
  } catch (error) {
    process.stderr.write(
      error instanceof CliError
        ? "invalid selected backend CLI arguments\n"
        : "selected backend admission failed; private inputs withheld\n",
    );
    if (process.argv[2] === "permissions" && error instanceof Error) {
      try {
        const stack = error.stack,
          code: unknown = Reflect.get(error, "code");
        const headers = [
          `${error.name}: ${error.message}\n`,
          ...(typeof code === "string" ? [`${error.name} [${code}]: ${error.message}\n`] : []),
        ];
        const header = headers.find((value) => stack?.startsWith(value));
        const frame = header ? (stack?.slice(header.length).split("\n")[0] ?? "") : "";
        // Only the first actual caller frame, never text in the error message
        // or a later frame used to replace a foreign/missing throw location.
        for (const file of [
          "tools/selected-backend-ci/runtime.ts",
          "tools/selected-backend-ci/admission.ts",
          "tools/selected-backend-ci/io.ts",
          "scripts/run-selected-backend-e2e.ts",
        ]) {
          const path = `${root}/${file}:`,
            direct = `    at ${path}`,
            named = `(${path}`;
          const offset = frame.startsWith(direct)
            ? direct.length
            : frame.includes(named) && /^ {4}at [^()]+ $/.test(frame.slice(0, frame.indexOf(named)))
              ? frame.indexOf(named) + named.length
              : -1;
          const numbers =
            offset >= 0 ? /^([1-9][0-9]{0,3}):[1-9][0-9]{0,3}\)?$/.exec(frame.slice(offset)) : null;
          if (numbers?.[1]) {
            process.stderr.write(`selected permissions failure at ${file}:${numbers[1]}\n`);
            break;
          }
        }
      } catch {
        // Malformed/unavailable stack evidence stays unknown; keep the verdict.
      }
    }
    process.exitCode = error instanceof CliError ? 2 : 1;
  }
}
