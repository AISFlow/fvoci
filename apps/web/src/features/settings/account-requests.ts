// The account settings page's requests, framework-neutral (the React and Vue
// account pages call the same functions). A write that changes a cached
// query refreshes it through the caller's query client.
import type { QueryClient } from "@tanstack/query-core";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import type { PasswordChangeInput, ProfileNameInput, WithdrawInput } from "@/lib/contracts";
import { erasureRecoveryHash } from "@/lib/erasure-hash";
import { identitiesQuery, meQuery, mfaStatusQuery } from "@/lib/queries";

export type MfaStatusOutput = components["schemas"]["MfaStatusOutput"];
export type MfaSetupOutput = components["schemas"]["MfaSetupOutput"];
export type MfaSetupInput = components["schemas"]["MfaSetupBody"];
export type MfaDisableInput = components["schemas"]["MfaDisableBody"];

/** Downloads the signed-in user's export archive as `fvoci-export.zip`. */
export async function downloadMeExport(): Promise<void> {
  const blob = await ensureOk(await api.GET("/api/v1/me/export", { parseAs: "blob" }));
  const href = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = href;
  link.download = "fvoci-export.zip";
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}

export async function saveProfileName(
  queryClient: QueryClient,
  input: ProfileNameInput,
): Promise<void> {
  await ensureOk(await api.PATCH("/api/v1/auth/me", { body: input }));
  await queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
}

export async function sendEmailVerification(email: string): Promise<void> {
  await ensureOk(await api.POST("/api/v1/auth/magic-link", { body: { email } }));
}

export async function requestEmailChange(newEmail: string): Promise<void> {
  await ensureOk(await api.PATCH("/api/v1/auth/email", { body: { newEmail } }));
}

export async function changePassword(
  queryClient: QueryClient,
  input: PasswordChangeInput,
): Promise<void> {
  await ensureOk(await api.PATCH("/api/v1/auth/password", { body: input }));
  await queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
}

/**
 * Schedules the account's erasure and returns the cancel page to load. The
 * cancel token travels in the fragment, so it never reaches a server log.
 * The response already cleared the session cookie; the caller leaves with a
 * full load, which drops every cached query of the withdrawn account.
 */
export async function withdrawAccount(input: WithdrawInput): Promise<string> {
  const result = await ensureOk(await api.POST("/api/v1/auth/withdraw", { body: input }));
  return `/cancel-withdraw#${erasureRecoveryHash({
    token: result.cancelToken,
    eraseAt: result.eraseAt,
    mailSent: result.mailSent,
  })}`;
}

export async function unlinkIdentity(queryClient: QueryClient, provider: string): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/auth/oidc/{provider}/unlink", {
      params: { path: { provider } },
    }),
  );
  await queryClient.invalidateQueries({ queryKey: identitiesQuery.queryKey });
}

/**
 * Starts TOTP enrolment. The answer holds the secret and the recovery codes:
 * callers keep it in the component that shows it, never in a query cache.
 */
export async function setUpMfa(input: MfaSetupInput): Promise<MfaSetupOutput> {
  return ensureOk(await api.POST("/api/v1/auth/mfa/setup", { body: input }));
}

export async function enableMfa(queryClient: QueryClient, code: string): Promise<void> {
  await ensureOk(await api.POST("/api/v1/auth/mfa/enable", { body: { code } }));
  await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
}

export async function disableMfa(queryClient: QueryClient, input: MfaDisableInput): Promise<void> {
  await ensureOk(await api.POST("/api/v1/auth/mfa/disable", { body: input }));
  await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
}
