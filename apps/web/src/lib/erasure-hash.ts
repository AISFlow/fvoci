// Adapted from fvoci/FVOCI apps/web/src/features/auth/cancel-withdraw.tsx
// The cancel token travels in the URL fragment so it never reaches server logs.

export interface ErasureFragment {
  token: string | null;
  eraseAt: string | null;
  scheduled: boolean;
  mailSent: boolean | null;
}

export function parseErasureHash(hash: string): ErasureFragment {
  const params = new URLSearchParams(hash.startsWith("#") ? hash.slice(1) : hash);
  const token = params.get("token");
  const eraseAt = params.get("eraseAt");
  const mail = params.get("mailSent");
  return {
    token: token !== null && token.length > 0 ? token : null,
    eraseAt: eraseAt !== null && !Number.isNaN(Date.parse(eraseAt)) ? eraseAt : null,
    scheduled: params.get("scheduled") === "1",
    mailSent: mail === "1" ? true : mail === "0" ? false : null,
  };
}

export function erasureRecoveryHash(input: {
  token: string;
  eraseAt: string;
  mailSent: boolean;
}): string {
  const params = new URLSearchParams();
  params.set("token", input.token);
  params.set("eraseAt", input.eraseAt);
  params.set("scheduled", "1");
  params.set("mailSent", input.mailSent ? "1" : "0");
  return params.toString();
}
