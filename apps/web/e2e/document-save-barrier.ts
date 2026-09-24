/** Wiki metadata PATCH is fire-and-forget on blur; local input value is not durability. */

export type DocumentPatchBody = {
  title?: string;
  icon?: string | null;
  status?: string;
};

export type DocumentIds = {
  workspaceId: string;
  documentId: string;
};

export function documentResourcePath(ids: DocumentIds): string {
  return `/api/v1/workspaces/${ids.workspaceId}/documents/${ids.documentId}`;
}

export function isDocumentResourceUrl(url: string, ids: DocumentIds): boolean {
  try {
    return new URL(url).pathname === documentResourcePath(ids);
  } catch {
    return false;
  }
}

export function patchBodyMatches(actual: unknown, expected: DocumentPatchBody): boolean {
  if (actual === null || typeof actual !== "object" || Array.isArray(actual)) {
    return false;
  }
  const record = actual as Record<string, unknown>;
  for (const [key, value] of Object.entries(expected) as [keyof DocumentPatchBody, unknown][]) {
    if (!Object.prototype.hasOwnProperty.call(record, key)) {
      return false;
    }
    if (!Object.is(record[key], value)) {
      return false;
    }
  }
  return true;
}

export function parseJsonBody(raw: string | null | undefined): unknown {
  if (raw === null || raw === undefined || raw === "") {
    return undefined;
  }
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

export function isSuccessfulMatchingDocumentPatch(args: {
  method: string;
  url: string;
  ok: boolean;
  workspaceId: string;
  documentId: string;
  requestBody: unknown;
  responseBody: unknown;
  expected: DocumentPatchBody;
}): boolean {
  return (
    args.method === "PATCH" &&
    args.ok &&
    isDocumentResourceUrl(args.url, {
      workspaceId: args.workspaceId,
      documentId: args.documentId,
    }) &&
    patchBodyMatches(args.requestBody, args.expected) &&
    patchBodyMatches(args.responseBody, args.expected)
  );
}

export function createHoldGate(): { held: Promise<void>; release: () => void } {
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { held, release };
}
