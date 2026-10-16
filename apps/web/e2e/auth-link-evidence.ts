import { readJson, flowSchemas } from "./helpers";
import { createHash } from "node:crypto";
import { SQL } from "bun";
import { expect, type Page } from "@playwright/test";

// Fixture updates affect only the runner's isolated database. Requests and
// token issuance/consumption still use the production Rust handlers.
export async function authSql(sql: string): Promise<string> {
  const url = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!url || !container) {
    throw new Error("isolated PostgreSQL fixture is required");
  }
  if (!/^\/fvoci_e2e_[a-f0-9]{16}$/.test(new URL(url).pathname))
    throw new Error("isolated PostgreSQL fixture database is required");
  const connection = new SQL(url, { max: 1, connectionTimeout: 10 });
  try {
    // Simple protocol + raw fields retain PostgreSQL's full timestamp text
    // and scalar spelling (including microseconds and t/f), as psql -At did.
    const rows: unknown = await connection.unsafe(sql).simple().raw();
    if (!Array.isArray(rows)) throw new Error("invalid PostgreSQL fixture rows");
    return rows
      .map((row: unknown) => {
        if (!Array.isArray(row)) throw new Error("invalid PostgreSQL fixture row");
        return row
          .map((value: unknown) => {
            if (value === null) return "";
            if (!Buffer.isBuffer(value)) throw new Error("invalid PostgreSQL fixture field");
            return value.toString();
          })
          .join("|");
      })
      .join("\n")
      .trim();
  } catch {
    throw new Error("isolated PostgreSQL auth fixture query failed");
  } finally {
    await connection.close();
  }
}

export function tokenHash(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

export async function expectVueAuth(page: Page): Promise<void> {
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
}

// Use the mounted production router to reproduce same-route query changes;
// no handler, network response or API client is replaced.
export async function navigateAuthQuery(page: Page, path: string): Promise<void> {
  await page.evaluate(async (target) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        config: {
          globalProperties: {
            $router: {
              push: (to: string) => Promise<unknown>;
            };
          };
        };
      };
    };
    await root.__vue_app__.config.globalProperties.$router.push(target);
  }, path);
}

export async function rejectMagicVariants(
  page: Page,
  path: string,
  token: string,
  submit: () => Promise<void>,
): Promise<void> {
  const hash = tokenHash(token);
  const expiry = await authSql(
    `SELECT expires_at FROM fvoci.magic_tokens WHERE token_hash = '${hash}'`,
  );
  expect(expiry).not.toBe("");
  // Opening the original URL never consumes the token.
  await page.goto(`${path}?token=${token}`);
  await expectVueAuth(page);
  expect(
    await authSql(`SELECT count(*) FROM fvoci.magic_tokens WHERE token_hash = '${hash}'`),
  ).toBe("1");
  await navigateAuthQuery(page, path);
  await expect(page.getByRole("alert")).toBeVisible();
  await navigateAuthQuery(page, `${path}?token=${token}`);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(
    await authSql(`SELECT count(*) FROM fvoci.magic_tokens WHERE token_hash = '${hash}'`),
  ).toBe("1");
  const tampered = `${token.slice(0, -1)}${token.endsWith("A") ? "B" : "A"}`;
  await navigateAuthQuery(page, `${path}?token=${tampered}`);
  await submit();
  await expect(page.getByRole("alert")).toContainText("링크가 만료되었거나 이미 사용되었습니다");
  await expect(page).toHaveURL(new RegExp(`${path}\\?token=`));
  expect(
    await authSql(`SELECT count(*) FROM fvoci.magic_tokens WHERE token_hash = '${hash}'`),
  ).toBe("1");
  await navigateAuthQuery(page, `${path}?token=${token}`);
  // A rejected previous token must not hide a new token's action.
  await expect(page.getByRole("alert")).toHaveCount(0);
  await authSql(
    `UPDATE fvoci.magic_tokens SET expires_at = now() - interval '1 second' WHERE token_hash = '${hash}'`,
  );
  try {
    await page.goto(`${path}?token=${token}`);
    await submit();
    await expect(page.getByRole("alert")).toContainText("링크가 만료되었거나 이미 사용되었습니다");
  } finally {
    await authSql(
      `UPDATE fvoci.magic_tokens SET expires_at = '${expiry}'::timestamptz WHERE token_hash = '${hash}'`,
    );
  }
}

export async function expectSpentMagic(page: Page, endpoint: string, token: string): Promise<void> {
  expect(
    await authSql(
      `SELECT count(*) FROM fvoci.magic_tokens WHERE token_hash = '${tokenHash(token)}'`,
    ),
  ).toBe("0");
  const response = await page.request.post(endpoint, { data: { token } });
  expect(response.status()).toBe(400);
  expect((await readJson(response, flowSchemas.error)).code).toBe("magic_invalid");
}
