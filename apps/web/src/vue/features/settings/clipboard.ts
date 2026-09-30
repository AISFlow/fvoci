/** Clipboard write used by invite links, tokens, webhook secrets, ICS, and SSO. */
export async function copyText(value: string): Promise<void> {
  const capabilities: { clipboard?: Clipboard } = navigator;
  const clipboard = capabilities.clipboard;
  if (clipboard && typeof clipboard.writeText === "function") {
    await clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}
