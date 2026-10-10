import { describe, expect, test } from "bun:test";
import { parseArgv, type OptionSpec } from "./argv.ts";

const SPECS: readonly OptionSpec[] = [
  { flag: "--mode", dest: "mode", required: true, choices: ["a", "b"] },
  { flag: "--value", dest: "value" },
];
const ok = (values: Record<string, string>) => ({ kind: "values" as const, values });
const error = (message: string) => ({ kind: "error" as const, message });
const help = { kind: "help" as const };

describe("parseArgv", () => {
  test("both value forms, last valid value wins", () => {
    expect(parseArgv(["--mode", "a", "--value=x"], SPECS)).toEqual(ok({ mode: "a", value: "x" }));
    expect(parseArgv(["--mode", "a", "--mode", "b"], SPECS)).toEqual(ok({ mode: "b" }));
    expect(parseArgv(["--mode", "a", "--value", ""], SPECS)).toEqual(ok({ mode: "a", value: "" }));
  });

  test("an invalid choice fails where it occurs and is never replaced", () => {
    const invalid = error("argument --mode: invalid choice: 'z' (choose from a, b)");
    expect(parseArgv(["--mode", "z", "--mode", "a"], SPECS)).toEqual(invalid);
    expect(parseArgv(["--mode", "a", "--mode", "z"], SPECS)).toEqual(invalid);
    expect(parseArgv(["--mode=z", "--mode", "a"], SPECS)).toEqual(invalid);
    expect(parseArgv(["--mode", "z", "--help"], SPECS)).toEqual(invalid);
    expect(parseArgv(["--help", "--mode", "z"], SPECS)).toEqual(help);
    expect(parseArgv(["-h"], SPECS)).toEqual(help);
  });

  test("help takes no attached value", () => {
    for (const token of ["--help=bad", "--help=", "-hbad", "-hh", "-h=x", "--he"]) {
      expect(parseArgv(["--mode", "a", token], SPECS), token).toEqual(
        error(`unrecognized arguments: ${token}`),
      );
    }
  });

  test("a value starting with - is a missing value", () => {
    for (const value of ["-1", "-1x", "-.5", "-١", "-１", "-x y", "-", "--", "--mode"]) {
      expect(parseArgv(["--mode", "a", "--value", value], SPECS), value).toEqual(
        error("argument --value: expected one argument"),
      );
    }
    expect(parseArgv(["--mode", "a", "--value=-1"], SPECS)).toEqual(
      error("argument --value: expected one argument"),
    );
  });

  test("abbreviations, extras and missing options are refused", () => {
    expect(parseArgv(["--mo", "a"], SPECS)).toEqual(error("unrecognized arguments: --mo"));
    expect(parseArgv(["--mode", "a", "x"], SPECS)).toEqual(error("unrecognized arguments: x"));
    expect(parseArgv(["--mode", "a", "--"], SPECS)).toEqual(error("unrecognized arguments: --"));
    expect(parseArgv(["--mode"], SPECS)).toEqual(error("argument --mode: expected one argument"));
    expect(parseArgv(["--value", "x"], SPECS)).toEqual(
      error("the following arguments are required: --mode"),
    );
  });
});
