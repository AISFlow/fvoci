/**
 * Patches: what a write sends, and what it does to the thing it is sent for.
 *
 * A store applies a patch to its own copy at once (the optimistic update)
 * and sends the same patch to the backend; the in-memory backend applies it
 * with the same functions. Every function returns a new object and leaves
 * its argument alone.
 */
import { sameRecurrence } from '../logic/recurrence.ts';
import {
  createBoardView,
  createPatchesFromBlocks,
  createPatchesFromContainers,
  type BlockPatch,
  type Container,
  type ContainerPatch,
  type BoardView,
  type Card,
} from '../logic/wire.ts';

import type { CardPatch } from './backend.ts';

/** The patch that turns `prev` into `next`, and the one that turns it back. */
export function cardPatches(next: Card, prev: Card): [CardPatch, CardPatch] {
  const forward: CardPatch = {};
  const backward: CardPatch = {};
  if (next.parentId !== prev.parentId) {
    forward.parentId = next.parentId;
    backward.parentId = prev.parentId;
  }
  if (next.title !== prev.title) {
    forward.title = next.title;
    backward.title = prev.title;
  }
  const nextIcon = next.fields.icon ?? '';
  const prevIcon = prev.fields.icon ?? '';
  if (nextIcon !== prevIcon) {
    forward.icon = nextIcon;
    backward.icon = prevIcon;
  }
  const before = prev.fields.properties;
  const after = next.fields.properties;
  const same = (first: unknown, second: unknown): boolean =>
    first === second || JSON.stringify(first) === JSON.stringify(second);
  for (const id of new Set([...Object.keys(before), ...Object.keys(after)])) {
    const was = before[id];
    const is = after[id];
    if (same(was, is)) continue;
    if (is === undefined) (forward.deletedProperties ??= []).push(id);
    else (forward.updatedProperties ??= {})[id] = is;
    if (was === undefined) (backward.deletedProperties ??= []).push(id);
    else (backward.updatedProperties ??= {})[id] = was;
  }
  if (!sameRecurrence(next.fields.recurrence, prev.fields.recurrence)) {
    forward.recurrence = next.fields.recurrence ?? null;
    backward.recurrence = prev.fields.recurrence ?? null;
  }
  return [forward, backward];
}

export function isEmptyCardPatch(patch: CardPatch): boolean {
  return (
    patch.parentId === undefined &&
    patch.title === undefined &&
    patch.icon === undefined &&
    patch.recurrence === undefined &&
    Object.keys(patch.updatedProperties ?? {}).length === 0 &&
    (patch.deletedProperties ?? []).length === 0
  );
}

export function applyCardPatch<T extends Card>(card: T, patch: CardPatch): T {
  const properties = { ...card.fields.properties, ...structuredClone(patch.updatedProperties) };
  for (const id of patch.deletedProperties ?? []) {
    // eslint-disable-next-line @typescript-eslint/no-dynamic-delete -- a patch names the keys to drop
    delete properties[id];
  }
  const fields = { ...card.fields, icon: patch.icon ?? card.fields.icon, properties };
  if (patch.recurrence !== undefined) {
    // Set anew, it repeats anew: the card made before is no longer its next.
    if (!sameRecurrence(patch.recurrence ?? undefined, card.fields.recurrence)) {
      delete fields.recurredTo;
    }
    if (patch.recurrence === null) delete fields.recurrence;
    else fields.recurrence = structuredClone(patch.recurrence);
  }
  return {
    ...card,
    parentId: patch.parentId === undefined ? card.parentId : (patch.parentId ?? ''),
    title: patch.title ?? card.title,
    fields,
  };
}

/** The patch that turns the view `prev` into `next`, and the one that turns it back. */
export function viewPatches(next: BoardView, prev: BoardView): [BlockPatch, BlockPatch] {
  const [forward, backward] = createPatchesFromBlocks(next, prev);
  const changed = (patch: BlockPatch): BlockPatch => {
    // `createPatchesFromBlocks` compares by identity; a copy that says the same is no change.
    const fields = (other: BoardView): Record<string, unknown> => ({ ...other.fields });
    const updatedFields: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(patch.updatedFields ?? {})) {
      const before = patch === forward ? fields(prev)[key] : fields(next)[key];
      if (JSON.stringify(before) !== JSON.stringify(value)) updatedFields[key] = value;
    }
    return { ...patch, updatedFields };
  };
  return [changed(forward), changed(backward)];
}

export function isEmptyBlockPatch(patch: BlockPatch): boolean {
  return (
    patch.title === undefined &&
    Object.keys(patch.updatedFields ?? {}).length === 0 &&
    (patch.deletedFields ?? []).length === 0
  );
}

export function applyViewPatch(view: BoardView, patch: BlockPatch): BoardView {
  const fields: Record<string, unknown> = {
    ...view.fields,
    ...structuredClone(patch.updatedFields),
  };
  for (const key of patch.deletedFields ?? []) {
    // eslint-disable-next-line @typescript-eslint/no-dynamic-delete -- a patch names the keys to drop
    delete fields[key];
  }
  return createBoardView({ ...view, title: patch.title ?? view.title, fields });
}

/** The patch that turns the board `prev` into `next`, and the one that turns it back. */
export function containerPatches(
  next: Container,
  prev: Container,
): [ContainerPatch, ContainerPatch] {
  return createPatchesFromContainers(next, prev);
}

export function isEmptyContainerPatch(patch: ContainerPatch): boolean {
  return Object.values(patch).every((value: unknown) => {
    if (Array.isArray(value)) return value.length === 0;
    if (typeof value === 'object' && value !== null) return Object.keys(value).length === 0;
    return value === undefined;
  });
}

export function applyContainerPatch(board: Container, patch: ContainerPatch): Container {
  let cardProperties = board.cardProperties.map((template) => ({
    ...template,
    options: template.options.map((option) => ({ ...option })),
  }));
  for (const template of structuredClone(patch.updatedCardProperties ?? [])) {
    const at = cardProperties.findIndex((existing) => existing.id === template.id);
    if (at === -1) cardProperties.push(template);
    else cardProperties[at] = template;
  }
  if (patch.deletedCardProperties?.length) {
    cardProperties = cardProperties.filter(
      (template) => !patch.deletedCardProperties?.includes(template.id),
    );
  }
  const properties = {
    ...board.properties,
    ...(patch.updatedProperties as Container['properties'] | undefined),
  };
  for (const key of patch.deletedProperties ?? []) {
    // eslint-disable-next-line @typescript-eslint/no-dynamic-delete -- a patch names the keys to drop
    delete properties[key];
  }
  return {
    ...board,
    visibility: patch.visibility ?? board.visibility,
    minimumRole: patch.minimumRole ?? board.minimumRole,
    title: patch.title ?? board.title,
    description: patch.description ?? board.description,
    icon: patch.icon ?? board.icon,
    showDescription: patch.showDescription ?? board.showDescription,
    dueDatePropertyId:
      patch.dueDatePropertyId === undefined
        ? board.dueDatePropertyId
        : (patch.dueDatePropertyId ?? null),
    doneWhen:
      patch.doneWhen === undefined
        ? board.doneWhen
        : patch.doneWhen === null
          ? null
          : structuredClone(patch.doneWhen),
    properties,
    cardProperties,
  };
}
