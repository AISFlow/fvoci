/**
 * Boards, views, cards and content blocks with fixed, recognisable content
 * for the tests of the logic: every test builds on the same few, so that what
 * a case expects can be read against them.
 */
import type { DateValue } from '$lib/logic/dates.ts';
import {
  appendCardContent,
  createBoard,
  createBoardView,
  createCard,
  createCheckboxBlock,
  createCommentBlock,
  createDividerBlock,
  createFilterGroup,
  createImageBlock,
  createTextBlock,
  formatDateValue,
  setCardProperty,
  type Block,
  type Container,
  type BoardView,
  type Card,
  type CheckboxBlock,
  type CommentBlock,
  type DividerBlock,
  type ImageBlock,
  type StoredDateValue,
  type TextBlock,
} from '$lib/logic/wire.ts';

/** A board with the default select property and three more, each with one option. */
export function createTestBoard(): Container {
  const board = createBoard();
  board.title = 'board title';
  board.description = 'description';
  board.showDescription = true;
  board.icon = 'i';
  for (let index = 1; index <= 3; index += 1) {
    board.cardProperties.push({
      id: `property${index}`,
      name: `Property ${index}`,
      type: 'select',
      options: [{ id: 'value1', label: 'value 1', color: 'brown' }],
    });
  }
  return board;
}

function createTestView(viewType: 'board' | 'table', board?: Container): BoardView {
  const view = createBoardView({
    ...createBoardView(),
    fields: {
      viewType,
      groupBy: 'property1',
      hiddenGroups: ['value1'],
      cardOrder: ['card1', 'card2', 'card3'],
      sort: [
        { propertyId: 'property1', reversed: true },
        { propertyId: 'property2', reversed: false },
      ],
      columnWidths: { column1: 100, column2: 200 },
      filter: createFilterGroup({
        operation: 'and',
        filters: [{ propertyId: 'property1', condition: 'includes', values: ['value1'] }],
      }),
    },
  });
  view.containerId = board ? board.id : 'board';
  view.title = 'view title';
  return view;
}

export const createTestBoardView = (board?: Container): BoardView => createTestView('board', board);
export const createTestTableView = (board?: Container): BoardView => createTestView('table', board);

export function createTestCard(board?: Container): Card {
  const card = createCard({ ...createCard(), fields: { icon: 'i' } });
  card.containerId = board ? board.id : 'board';
  card.title = 'title';
  setCardProperty(card, 'property1', 'value1');
  return card;
}

function addToCard<T extends Block>(block: T, card: Card, isContent = true): T {
  block.parentId = card.id;
  block.containerId = card.containerId;
  if (isContent) appendCardContent(card, block.id);
  return block;
}

export function createTestComment(card: Card): CommentBlock {
  const block = addToCard(createCommentBlock(), card, false);
  block.title = 'title';
  return block;
}

export function createTestText(card: Card): TextBlock {
  const block = addToCard(createTextBlock(), card);
  block.title = 'title';
  return block;
}

export function createTestImage(card: Card): ImageBlock {
  return addToCard(createImageBlock({ ...createImageBlock(), fields: { fileId: 'fileId' } }), card);
}

export function createTestDivider(card: Card): DividerBlock {
  const block = addToCard(createDividerBlock(), card);
  block.title = 'title';
  return block;
}

export function createTestCheckbox(card: Card): CheckboxBlock {
  const block = addToCard(createCheckboxBlock(), card);
  block.title = 'title';
  return block;
}

/** What is stored for a date value that is not empty. */
export function storedDate(value: DateValue): StoredDateValue {
  const stored = formatDateValue(value);
  if (!stored) throw new Error('an empty date value is not stored');
  return stored;
}
