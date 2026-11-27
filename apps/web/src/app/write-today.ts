/**
 * The diary the hub of today was asked to write today in: its page, opened
 * next, puts the caret in today's day once (where the pane is wide enough for
 * a keyboard to be the likely one). The ask is the next diary page's, whichever
 * diary it shows, and only a moment after it was made: one that went astray
 * (the page opened was another, or none) puts no caret anywhere later.
 */
/** How long an ask waits for the page it opens. */
export const ASK_WAITS = 5_000;

let asked: { diaryId: string; at: number } | undefined;

export function askToWriteToday(diaryId: string, now = Date.now()): void {
  asked = { diaryId, at: now };
}

/** Whether `diaryId` was asked to be written in today just now; the ask is taken in any case. */
export function takeWriteToday(diaryId: string, now = Date.now()): boolean {
  const ask = asked;
  asked = undefined;
  return ask?.diaryId === diaryId && now - ask.at < ASK_WAITS;
}
