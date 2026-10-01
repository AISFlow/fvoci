export async function persistThenCreate<T>(
  persistNow: (() => void | Promise<void>) | undefined,
  create: () => T | Promise<T>,
): Promise<T> {
  if (!persistNow) throw new Error("Revision save requires a live persist barrier");
  await persistNow();
  return await create();
}
