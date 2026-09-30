/** Evaluate trusted, locally compiled SFC fixtures without Function/eval. */
export async function evaluateTestFunction(
  parameters: string[],
  body: string,
): Promise<(...values: unknown[]) => unknown> {
  const code = `export function fixture(${parameters.join(",")}) {\n${body}\n}`;
  const loaded: unknown = await import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );

  if (
    typeof loaded !== "object" ||
    loaded === null ||
    !("fixture" in loaded) ||
    typeof loaded.fixture !== "function"
  ) {
    throw new Error("Compiled SFC fixture did not export a function");
  }
  return loaded.fixture as (...values: unknown[]) => unknown;
}
