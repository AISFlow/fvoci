/**
 * What a change message says of a card's or a page's body in its place (the
 * `summary` of `blockChanged`, `contracts/websocket.md` 3): whether there is
 * one, its task items and its preview, as a list of cards gives them. The
 * memory backend makes it of the body it holds, as the server does.
 */
import { countTasks } from '../editor/body-text.ts';
import { cardPreviewOf, type CardPreview } from '../editor/card-preview.ts';
import type { DocNode } from '../editor/schema.ts';

export interface BodySummary {
  hasBody: boolean;
  tasksTotal: number;
  tasksDone: number;
  preview: CardPreview;
}

/** The summary of `body`; that of no body says there is none. */
export function summaryOfBody(body: DocNode | null | undefined): BodySummary {
  const tasks = body ? countTasks(body) : { total: 0, checked: 0 };
  return {
    hasBody: body !== null && body !== undefined,
    tasksTotal: tasks.total,
    tasksDone: tasks.checked,
    preview: cardPreviewOf(body),
  };
}
