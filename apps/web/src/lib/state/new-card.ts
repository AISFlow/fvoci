/**
 * What the header's "New" does on the view that is shown. The header and
 * the board page know nothing of a view's inside; a view that makes a new
 * card its own way names a maker here for as long as it is shown, and the
 * board page asks for it first: a calendar makes the card on the day
 * picked, a Gantt chart on today (or the first day in sight). For any other
 * view, or one that cannot make a card its way (its dates cannot be set),
 * there is none, and the page makes a card as on every view.
 */

/** Makes a card the view's way, and opens it. */
export type NewCardMaker = () => Promise<void>;

const makers = new Map<string, NewCardMaker>();

/** Names the maker of a view; the returned function takes it back. */
export function registerNewCard(viewId: string, maker: NewCardMaker): () => void {
  makers.set(viewId, maker);
  return () => {
    if (makers.get(viewId) === maker) makers.delete(viewId);
  };
}

/** The maker a view named, if any. */
export function newCardMaker(viewId: string): NewCardMaker | undefined {
  return makers.get(viewId);
}
