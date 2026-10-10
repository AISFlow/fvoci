// Environment passed to Playwright and the SMTP sink. Preparation secrets and
// the Jest worker id stay with the parent process.

export function playwrightChildEnv(base: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...base };
  for (const key of Object.keys(env)) {
    if (
      key === "DATABASE_URL" ||
      key === "MEILI_MASTER_KEY" ||
      key === "JEST_WORKER_ID" ||
      key.startsWith("PASSWORD")
    )
      delete env[key];
  }
  return env;
}
