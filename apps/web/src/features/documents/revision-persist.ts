export async function persistThenCreate(
  persistNow: (() => void | Promise<void>) | undefined,
  create: () => void,
): Promise<void> {
  await persistNow?.();
  create();
}
