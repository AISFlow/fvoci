import { t, tProblemTitle } from "@fvoci/i18n";
import createClient from "openapi-fetch";
import type { components, paths } from "@/generated/api";
import { CONSENT_PATH, consentUrl, isConsentRequired } from "@/lib/consent";

/**
 * Source 428 branch: a signed-in user with pending required legal documents
 * gets `consent_required` on every gated request. Send them to the prompt with
 * the current page as `returnTo`. The request never settles, so callers do not
 * race the page change with their own error redirects (e.g. me → /login).
 */
export async function consentGate(response: Response): Promise<Response> {
  if (response.status !== 428 || window.location.pathname === CONSENT_PATH) {
    return response;
  }
  const body: unknown = await response
    .clone()
    .json()
    .catch(() => null);
  if (!isConsentRequired(response.status, body)) return response;
  window.location.assign(consentUrl(window.location));
  return new Promise<Response>(() => {});
}

export const api = createClient<paths>({
  credentials: "include",
  fetch: async (input) => consentGate(await globalThis.fetch(input)),
});

type ProblemBody = components["schemas"]["ProblemResponse"];

export class ProblemError extends Error {
  readonly status: number;
  readonly title: string;
  readonly titleKnown: boolean;
  readonly code?: string;
  /** `params.code` of the body: the specific reason under a general `code`. */
  readonly reason?: string;

  constructor(
    status: number,
    code?: string,
    params?: Record<string, string | number>,
    reason?: string,
  ) {
    const localized = code ? tProblemTitle(code, params) : t("error.http.fallback");
    super(localized);
    this.name = "ProblemError";
    this.status = status;
    this.title = localized;
    this.titleKnown = Boolean(code);
    this.code = code;
    this.reason = reason;
  }
}

/**
 * A stale or foreign pagination cursor. The server reports it as
 * `invalid_input` with `params.code = "invalid_cursor"` on most lists
 * (collections among them), and as a top-level `invalid_cursor` on comments
 * and task activity.
 */
export function isInvalidCursor(err: unknown): boolean {
  if (!(err instanceof ProblemError)) return false;
  return err.code === "invalid_cursor" || err.reason === "invalid_cursor";
}

/** The request itself was rejected as invalid, not just its cursor. */
export function isInvalidInput(err: unknown): boolean {
  return err instanceof ProblemError && err.code === "invalid_input" && !isInvalidCursor(err);
}

/** A failed load's message: the problem title, or the generic load failure. */
export function loadErrorMessage(error: unknown): string {
  return error instanceof ProblemError ? error.title : t("load.failed");
}

export function problemMessage(err: unknown, fallback: Parameters<typeof t>[0]): string {
  if (!(err instanceof ProblemError)) return t("error.network");
  return err.titleKnown ? err.title : t(fallback);
}

type ApiResult<T> = {
  data?: T;
  error?: ProblemBody;
  response: Response;
};

function problemReason(params: unknown): string | undefined {
  if (typeof params !== "object" || params === null) return undefined;
  const code = (params as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
}

export async function ensureOk<T>(result: ApiResult<T>): Promise<T> {
  if (result.error) {
    const status = result.response.status;
    const reason = problemReason(result.error.params);
    throw new ProblemError(status, result.error.code, undefined, reason);
  }
  if (result.data === undefined) {
    throw new ProblemError(500);
  }
  return result.data;
}
