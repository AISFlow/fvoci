import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { BrowserContext } from "@playwright/test";

/** API, SSE and collaboration always use the real server. Only this fixture's
 * built assets are intercepted, in this test's browser context. The real
 * document retains server CSP and the browser's network address space. */
export function buildCacheObserverFixture() {
  const dist = mkdtempSync(path.join(tmpdir(), "fvoci-cache-observer-"));
  try {
    execFileSync(
      "bun",
      ["--bun", "x", "--no-install", "vite", "build", "--mode=task-cache-e2e", `--outDir=${dist}`],
      {
        cwd: path.resolve(import.meta.dirname, ".."),
        env: { ...process.env, NODE_ENV: "production" },
        stdio: "pipe",
      },
    );
  } catch (error) {
    rmSync(dist, { recursive: true, force: true });
    throw error;
  }
  function entryScript(directory: string): string {
    const html = readFileSync(path.join(directory, "index.html"), "utf8");
    const script = /<script[^>]+src="([^"]+\.js)"/.exec(html)?.[1];
    if (!script) throw new Error("Cache fixture requires a single built entry script");
    return script;
  }
  let productionEntry: string;
  let fixtureEntry: string;
  try {
    const staticDirectory = process.env.FVOCI_STATIC_DIR;
    if (!staticDirectory) throw new Error("Cache fixture requires actual server static directory");
    productionEntry = entryScript(staticDirectory);
    fixtureEntry = entryScript(dist);
    const styles = (directory: string) =>
      [
        ...readFileSync(path.join(directory, "index.html"), "utf8").matchAll(
          /<link[^>]+rel="stylesheet"[^>]+href="([^"]+)"/g,
        ),
      ].map((match) => match[1]);
    if (JSON.stringify(styles(staticDirectory)) !== JSON.stringify(styles(dist))) {
      throw new Error("Cache fixture must preserve the production document's stylesheets");
    }
  } catch (error) {
    rmSync(dist, { recursive: true, force: true });
    throw error;
  }
  const served = new Map<
    string,
    {
      url: string;
      source: string;
      sourceSha256: string;
      servedSha256: string;
      securityHeaders: Record<string, string>;
    }
  >();
  return {
    evidence() {
      return {
        mode: "task-cache-e2e",
        productionEntry,
        fixtureEntry,
        assets: [...served.values()],
      };
    },
    async install(context: BrowserContext) {
      await context.route(
        (url) => url.pathname.startsWith("/assets/"),
        async (route) => {
          const pathname = new URL(route.request().url()).pathname;
          const file = path.join(dist, pathname === productionEntry ? fixtureEntry : pathname);
          if (!file.startsWith(`${dist}/`) || !existsSync(file)) {
            await route.continue();
            return;
          }
          // Preserve the real server's CSP and other response headers.
          const response = await route.fetch();
          const source = readFileSync(file);
          const body = file.endsWith(".js")
            ? source
                .toString("utf8")
                .replaceAll(path.basename(fixtureEntry), path.basename(productionEntry))
            : source;
          const securityHeaders = Object.fromEntries(
            Object.entries(response.headers()).filter(([key]) =>
              [
                "content-security-policy",
                "x-content-type-options",
                "referrer-policy",
                "permissions-policy",
                "cross-origin-resource-policy",
                "cross-origin-opener-policy",
              ].includes(key),
            ),
          );
          served.set(pathname, {
            url: pathname,
            source: path.relative(dist, file),
            sourceSha256: createHash("sha256").update(source).digest("hex"),
            servedSha256: createHash("sha256").update(body).digest("hex"),
            securityHeaders,
          });
          const contentType = file.endsWith(".js")
            ? "application/javascript"
            : file.endsWith(".css")
              ? "text/css"
              : file.endsWith(".html")
                ? "text/html"
                : undefined;
          // Fixture chunk hashes differ from the ordinary build, so the real
          // static server can return 404 for a fixture-only asset. Keep its
          // security headers, while supplying the existing fixture file/type.
          // Lazy chunks import entry exports. Use one URL/module identity.
          await route.fulfill({ response, body, status: 200, contentType });
        },
      );
    },
    dispose() {
      rmSync(dist, { recursive: true, force: true });
    },
  };
}
