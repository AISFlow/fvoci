import { sanitizeReasonCode, type Plan } from "./plan.ts";
import { pyDumps, type PyValue } from "./pyjson.ts";

// Byte formats the workflows and the gate read: the plan file, the
// GITHUB_OUTPUT step outputs and the one-line stdout summary.

export function renderPlanFile(plan: Plan): string {
  return pyDumps(plan, { indent: 2 }) + "\n";
}

/** `plan_json` is compact with sorted keys; job outputs are select_<job, - as _>. */
export function renderGithubOutput(plan: Plan, postgresMatrix: string | null): string {
  const lines = [
    `mode=${plan.mode}`,
    `reason_code=${sanitizeReasonCode(plan.reason_code)}`,
    `plan_ok=${plan.plan_ok ? "true" : "false"}`,
  ];
  for (const [job, meta] of Object.entries(plan.jobs)) {
    if (typeof meta.selected !== "boolean") throw new Error("job.selected must be bool");
    lines.push(`select_${job.replaceAll("-", "_")}=${meta.selected ? "true" : "false"}`);
  }
  let text = lines.join("\n") + "\n";
  text += `plan_json<<PLAN_EOF\n${pyDumps(plan, { compact: true, sortKeys: true })}\nPLAN_EOF\n`;
  if (postgresMatrix !== null) {
    if (postgresMatrix.includes("\n") || !postgresMatrix.startsWith('{"include":[')) {
      throw new Error("postgres_matrix must be one JSON object line with include");
    }
    text += `postgres_matrix<<POSTGRES_MATRIX_EOF\n${postgresMatrix}\nPOSTGRES_MATRIX_EOF\n`;
  }
  return text;
}

export function renderSummary(plan: Plan): string {
  return (
    pyDumps(
      new Map<string, PyValue>([
        ["mode", plan.mode],
        ["reason_code", plan.reason_code],
      ]),
    ) + "\n"
  );
}
