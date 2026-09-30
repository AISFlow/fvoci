// The instance-admin pages' writes, framework-neutral (the React and Vue
// admin pages call the same functions). The server answers 404 to anyone who
// is not an instance admin on every /admin route.
import type { QueryClient } from "@tanstack/query-core";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { legalDocQuery, legalVersionsQuery } from "@/lib/queries/admin";
import type { BrandingAssetKind } from "./settings-catalog";

export type AdminInstanceSettings = components["schemas"]["AdminInstanceSettingsOutput"];
export type LegalPublishInput = components["schemas"]["LegalPublishBody"];

export type AdminUserPatch = { instanceAdmin?: boolean; suspended?: boolean };

/** PATCHes the settings documents in `patch`; answers the whole admin view. */
export async function patchInstanceSettings(
  patch: Record<string, unknown>,
): Promise<AdminInstanceSettings> {
  return ensureOk(
    await api.PATCH("/api/v1/admin/instance-settings", {
      // The catalog form builds the body key by key; the server checks it strictly.
      body: patch,
    }),
  );
}

/**
 * Assets are not PATCH leaves: upload is a raw octet POST, clearing (`null`)
 * a DELETE. Answers the whole admin view.
 */
export async function saveBrandingAsset(
  kind: BrandingAssetKind,
  file: File | null,
): Promise<AdminInstanceSettings> {
  if (file === null) {
    return ensureOk(
      await api.DELETE("/api/v1/admin/branding/assets/{asset}", {
        params: { path: { asset: kind } },
      }),
    );
  }
  // Match the server's inclusive 512 KiB limit before allocating the byte copies.
  if (file.size > 512 * 1024) {
    throw new ProblemError(413, "invalid_input");
  }
  // The generated octet-stream schema is a byte array. Keep that real byte contract
  // through serialization instead of asserting that a File is an array.
  const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
  return ensureOk(
    await api.POST("/api/v1/admin/branding/assets/{asset}", {
      params: { path: { asset: kind } },
      body: bytes,
      bodySerializer: () => new Uint8Array(bytes),
      headers: { "Content-Type": "application/octet-stream" },
    }),
  );
}

export async function patchAdminUser(input: { userId: string } & AdminUserPatch): Promise<void> {
  await ensureOk(await api.PATCH("/api/v1/admin/users", { body: input }));
}

/** Source eraseUser: schedules the erasure; the cancel link is mailed to the user, never answered. */
export async function eraseAdminUser(userId: string): Promise<void> {
  await ensureOk(await api.POST("/api/v1/admin/users/erase", { body: { userId } }));
}

/** Source cancelEraseUser. */
export async function cancelAdminUserErasure(userId: string): Promise<void> {
  await ensureOk(await api.POST("/api/v1/admin/users/cancel-erase", { body: { userId } }));
}

export async function publishLegalDocument(
  queryClient: QueryClient,
  input: LegalPublishInput,
): Promise<void> {
  await ensureOk(await api.POST("/api/v1/admin/legal", { body: input }));
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: legalDocQuery(input.kind).queryKey }),
    queryClient.invalidateQueries({ queryKey: legalVersionsQuery(input.kind).queryKey }),
  ]);
}
