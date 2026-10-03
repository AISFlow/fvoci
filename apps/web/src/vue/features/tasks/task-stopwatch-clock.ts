/** Browser monotonic time only renders a server sample. It never starts,
 * closes, corrects or persists a segment, even while offline. */
export function anchoredElapsed(
  elapsedMilliseconds: number,
  runningSince: string | null | undefined,
  serverNow: string,
  receivedAt: number,
  monotonicNow: number,
): number {
  if (!runningSince) return elapsedMilliseconds;
  const sampled = Math.max(0, Date.parse(serverNow) - Date.parse(runningSince));
  return elapsedMilliseconds + sampled + Math.max(0, monotonicNow - receivedAt);
}

export function stopwatchText(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  return [Math.floor(seconds / 3600), Math.floor((seconds % 3600) / 60), seconds % 60]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}
