/**
 * Wire patch builders for blocks, card property definitions, and containers,
 * re-exported through `wire.ts`.
 */
import type { Block, BlockPatch } from './wire-blocks.ts';
import type { Container, ContainerPatch, PropertyTemplate } from './wire.ts';

type Members = object | null | undefined;

function keysOnlyIn(first: Members, second: Members): string[] {
  const other = new Set(Object.keys(second ?? {}));
  return Object.keys(first ?? {}).filter((key) => !other.has(key));
}

function changedMembers(
  from: Members,
  to: Members,
  skip: readonly string[],
): Record<string, unknown> {
  const fromRecord = (from ?? {}) as Record<string, unknown>;
  const toRecord = (to ?? {}) as Record<string, unknown>;
  const changed: Record<string, unknown> = {};
  for (const key of Object.keys(fromRecord)) {
    if (!skip.includes(key) && fromRecord[key] !== toRecord[key]) changed[key] = fromRecord[key];
  }
  return changed;
}

export function createPatchesFromBlocks(
  newBlock: Block,
  oldBlock: Block,
): [BlockPatch, BlockPatch] {
  return [
    {
      ...changedMembers(newBlock, oldBlock, ['fields']),
      updatedFields: changedMembers(newBlock.fields, oldBlock.fields, []),
      deletedFields: keysOnlyIn(oldBlock.fields, newBlock.fields),
    },
    {
      ...changedMembers(oldBlock, newBlock, ['fields']),
      updatedFields: changedMembers(oldBlock.fields, newBlock.fields, []),
      deletedFields: keysOnlyIn(newBlock.fields, oldBlock.fields),
    },
  ];
}

function templatesEqual(first: PropertyTemplate, second: PropertyTemplate): boolean {
  if (Object.keys(changedMembers(first, second, ['options'])).length > 0) return false;
  if (first.options.length !== second.options.length) return false;
  return first.options.every((option) => {
    const other = second.options.find((candidate) => candidate.id === option.id);
    return other !== undefined && Object.keys(changedMembers(option, other, [])).length === 0;
  });
}

function changedTemplates(
  templates: readonly PropertyTemplate[],
  others: readonly PropertyTemplate[],
): PropertyTemplate[] {
  return templates.filter((template) => {
    const other = others.find((candidate) => candidate.id === template.id);
    return !other || !templatesEqual(template, other);
  });
}

function missingTemplateIds(
  templates: readonly PropertyTemplate[],
  others: readonly PropertyTemplate[],
): string[] {
  return templates
    .filter((template) => !others.some((candidate) => candidate.id === template.id))
    .map((template) => template.id);
}

export function createCardPropertiesPatches(
  newCardProperties: readonly PropertyTemplate[],
  oldCardProperties: readonly PropertyTemplate[],
): [ContainerPatch, ContainerPatch] {
  return [
    {
      updatedCardProperties: changedTemplates(newCardProperties, oldCardProperties),
      deletedCardProperties: missingTemplateIds(oldCardProperties, newCardProperties),
    },
    {
      updatedCardProperties: changedTemplates(oldCardProperties, newCardProperties),
      deletedCardProperties: missingTemplateIds(newCardProperties, oldCardProperties),
    },
  ];
}

const BOARD_MEMBERS_WITH_OWN_PATCH = [
  'properties',
  'cardProperties',
  'dueDatePropertyId',
  'doneWhen',
];

const sameJson = (first: unknown, second: unknown): boolean =>
  (first ?? null) === (second ?? null) ||
  JSON.stringify(first ?? null) === JSON.stringify(second ?? null);

export function createPatchesFromContainers(
  newBoard: Container,
  oldBoard: Container,
): [ContainerPatch, ContainerPatch] {
  const [cardPropertiesPatch, cardPropertiesUndoPatch] = createCardPropertiesPatches(
    newBoard.cardProperties,
    oldBoard.cardProperties,
  );
  const dueChanged = (newBoard.dueDatePropertyId ?? null) !== (oldBoard.dueDatePropertyId ?? null);
  const doneChanged = !sameJson(newBoard.doneWhen, oldBoard.doneWhen);
  return [
    {
      ...changedMembers(newBoard, oldBoard, BOARD_MEMBERS_WITH_OWN_PATCH),
      ...cardPropertiesPatch,
      ...(dueChanged ? { dueDatePropertyId: newBoard.dueDatePropertyId ?? null } : {}),
      ...(doneChanged ? { doneWhen: newBoard.doneWhen ?? null } : {}),
      updatedProperties: changedMembers(newBoard.properties, oldBoard.properties, []),
      deletedProperties: keysOnlyIn(oldBoard.properties, newBoard.properties),
    },
    {
      ...changedMembers(oldBoard, newBoard, BOARD_MEMBERS_WITH_OWN_PATCH),
      ...cardPropertiesUndoPatch,
      ...(dueChanged ? { dueDatePropertyId: oldBoard.dueDatePropertyId ?? null } : {}),
      ...(doneChanged ? { doneWhen: oldBoard.doneWhen ?? null } : {}),
      updatedProperties: changedMembers(oldBoard.properties, newBoard.properties, []),
      deletedProperties: keysOnlyIn(newBoard.properties, oldBoard.properties),
    },
  ];
}
