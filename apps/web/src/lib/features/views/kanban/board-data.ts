/**
 * What the kanban and the gallery read from the board store for the face of
 * a card, and the changes the kanban makes to the options a board is
 * grouped by.
 */
import { DEFAULT_OPTION_COLOR } from '$lib/components/option-chip/colors.ts';
import type { PropertyUser } from '$lib/components/properties/index.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { subcardProgress, type SubcardProgress } from '$lib/logic/card-hierarchy.ts';
import { propertiesThatMeetFilterGroup } from '$lib/logic/filter.ts';
import type { Regional } from '$lib/logic/regional.ts';
import {
  createId,
  type Container,
  type BoardView,
  type Card,
  type PropertyOption,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';
import type { BoardStore, CardSummary } from '$lib/state/index.ts';

export interface ParentHint {
  number?: number;
  title: string;
}

export interface FaceHierarchy {
  progressOf: (cardId: string) => SubcardProgress;
  parentOf: (card: Card) => ParentHint | undefined;
}

/** What the face of every card of a view is drawn with. */
export interface FaceContext {
  templates: readonly PropertyTemplate[];
  shownProperties: readonly string[];
  locale: string;
  regional: Regional;
  users: readonly PropertyUser[];
  hierarchy?: FaceHierarchy;
}

const NO_TEMPLATES: readonly PropertyTemplate[] = [];

export function faceContextOf(board: BoardStore, view: BoardView): FaceContext {
  const templates = board.board?.cardProperties ?? NO_TEMPLATES;
  const { byId, childrenMap } = board.hierarchy;
  const statusProp = board.statusProperty;
  const untitled = m['KanbanCard.untitled']();
  return {
    templates,
    shownProperties: view.fields.shownProperties,
    locale: board.workspace.locale,
    regional: board.workspace.regional,
    users: board.workspace.users,
    hierarchy: {
      progressOf: (cardId) => subcardProgress(cardId, childrenMap, templates, statusProp),
      parentOf: (card) => {
        if (card.parentId === '') return undefined;
        const parent = byId.get(card.parentId);
        if (!parent) return undefined;
        return {
          ...(parent.number === undefined ? {} : { number: parent.number }),
          title: parent.title || untitled,
        };
      },
    },
  };
}

/** What a card list says about the body of a card. A card from elsewhere says nothing. */
export function badgesOf(card: Card): Pick<CardSummary, 'hasBody' | 'tasksTotal' | 'tasksDone'> {
  const summary: Partial<CardSummary> = card;
  return {
    hasBody: summary.hasBody ?? false,
    tasksTotal: summary.tasksTotal ?? 0,
    tasksDone: summary.tasksDone ?? 0,
  };
}

/** The value a card has for the property a board is grouped by, as the board store reads it. */
export function columnValue(card: Card, groupBy: string): string {
  const value = card.fields.properties[groupBy];
  return typeof value === 'string' ? value : '';
}

/**
 * The properties of a card made in a column: what the filter of the view
 * asks for, so that the view shows the card, and the value of the column.
 * The column of cards without a value sets none.
 */
export function propertiesForColumn(
  board: Container,
  view: BoardView,
  groupBy: string,
  optionId: string,
): Record<string, PropertyValue> {
  const met = propertiesThatMeetFilterGroup(view.fields.filter, board.cardProperties);
  const properties: Record<string, PropertyValue> = Object.fromEntries(
    Object.entries(met).filter(([propertyId]) => propertyId !== groupBy),
  );
  if (optionId !== '') properties[groupBy] = optionId;
  return properties;
}

/** The board with the options of one property changed. */
function withOptions(
  board: Container,
  propertyId: string,
  change: (options: PropertyOption[]) => PropertyOption[],
): Container {
  return {
    ...board,
    cardProperties: board.cardProperties.map((template) =>
      template.id === propertyId ? { ...template, options: change(template.options) } : template,
    ),
  };
}

/** Changes one option of a property: its name or its colour. */
export async function changeOption(
  board: BoardStore,
  propertyId: string,
  optionId: string,
  change: Partial<Pick<PropertyOption, 'label' | 'color'>>,
): Promise<void> {
  await board.changeBoard(
    (current) =>
      withOptions(current, propertyId, (options) =>
        options.map((option) => (option.id === optionId ? { ...option, ...change } : option)),
      ),
    'changeOption',
  );
}

/**
 * Deletes an option of a property. The cards that had it keep the stored
 * value, which now names nothing: they show under the column of cards
 * without a value, and undo gives them their column back.
 */
export async function deleteOption(
  board: BoardStore,
  propertyId: string,
  optionId: string,
): Promise<void> {
  await board.changeBoard(
    (current) =>
      withOptions(current, propertyId, (options) =>
        options.filter((option) => option.id !== optionId),
      ),
    'deleteOption',
  );
}

/**
 * Adds a group to a board view: a new option of the property it is grouped
 * by, shown as the last column. One undo takes both changes back. Returns
 * the identifier of the option.
 */
export async function addGroup(
  board: BoardStore,
  view: BoardView,
  propertyId: string,
  name: string,
  shownOptionIds: readonly string[],
): Promise<string> {
  const option: PropertyOption = {
    id: createId('block'),
    label: name,
    color: DEFAULT_OPTION_COLOR,
  };
  await board.inOneStep(async (groupId) => {
    await board.changeBoard(
      (current) => withOptions(current, propertyId, (options) => [...options, option]),
      'addOption',
      groupId,
    );
    await board.setViewFields(
      view.id,
      { visibleGroups: [...shownOptionIds, option.id] },
      'addOption',
      groupId,
    );
  });
  return option.id;
}
