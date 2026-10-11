// Minimal, fail-closed argv parsing for the CI selection CLIs.
//
// Accepts `--name VALUE` and `--name=VALUE` for the declared options and an
// exact `-h`/`--help`. Tokens are handled left to right: a value outside an
// option's choices is refused where it occurs (a later valid value cannot
// replace it), and help is honoured only when every earlier token was valid.
// Stricter than argparse on purpose: no option abbreviations, no attached
// values on help, and a value starting with `-` is a missing value.

export type OptionSpec = {
  /** The long option, e.g. `--workflow`. */
  flag: string;
  dest: string;
  required?: boolean;
  choices?: readonly string[];
};

export type ArgvResult =
  | { kind: "values"; values: Record<string, string> }
  | { kind: "help" }
  | { kind: "error"; message: string };

export function parseArgv(argv: readonly string[], specs: readonly OptionSpec[]): ArgvResult {
  const byFlag = new Map(specs.map((spec) => [spec.flag, spec]));
  const values: Record<string, string> = {};
  const error = (message: string): ArgvResult => ({ kind: "error", message });
  for (let i = 0; i < argv.length; i++) {
    const token = argv[i] as string;
    if (token === "-h" || token === "--help") return { kind: "help" };
    const eq = token.startsWith("--") ? token.indexOf("=") : -1;
    const flag = eq > 0 ? token.slice(0, eq) : token;
    const spec = byFlag.get(flag);
    if (!spec) return error(`unrecognized arguments: ${token}`);
    const value = eq > 0 ? token.slice(eq + 1) : argv[++i];
    if (value === undefined || value.startsWith("-")) {
      return error(`argument ${flag}: expected one argument`);
    }
    if (spec.choices && !spec.choices.includes(value)) {
      return error(
        `argument ${flag}: invalid choice: '${value}' (choose from ${spec.choices.join(", ")})`,
      );
    }
    values[spec.dest] = value;
  }
  const missing = specs.filter((spec) => spec.required && !Object.hasOwn(values, spec.dest));
  if (missing.length > 0) {
    return error(
      `the following arguments are required: ${missing.map((spec) => spec.flag).join(", ")}`,
    );
  }
  return { kind: "values", values };
}
