/**
 * The definitions of a board's properties changed through the board store,
 * on the in-memory backend: adding, naming, changing the kind, deleting, and
 * the options of a select. Each is one step for "undo".
 */
import { describe, expect, it } from 'vitest';

import {
  addProperty,
  changePropertyType,
  deleteProperty,
  optionCallbacks,
  renameProperty,
} from '$lib/features/card/property-actions.ts';
import { planTypeChange } from '$lib/features/card/property-change.ts';
import type { PropertyTemplate } from '$lib/logic/wire.ts';
import { BackendError } from '$lib/state/index.ts';

import { OWNER, openFirstBoard, settle, STATUS } from '../../state/support.ts';

async function open() {
  const made = await openFirstBoard();
  const { board, backend } = made;
  const stored = async (): Promise<PropertyTemplate[]> =>
    (await backend.container(board.boardId)).cardProperties;
  const definition = (id: string): PropertyTemplate | undefined =>
    board.board?.cardProperties.find((template) => template.id === id);
  const view = board.orderedViews[0];
  if (!view) throw new Error('no view');
  const cardNamed = (title: string) => {
    const card = board.cards.find((each) => each.title === title);
    if (!card) throw new Error(`no card ${title}`);
    return card;
  };
  return { ...made, stored, definition, view, cardNamed };
}

describe('adding a property', () => {
  it('puts a property named after its kind at the end, on the board and on the server', async () => {
    const { board, stored, view } = await open();
    const id = await addProperty(board, view, 'number', 'Number');
    expect(board.board?.cardProperties.at(-1)).toEqual({
      id,
      name: 'Number',
      type: 'number',
      options: [],
    });
    expect((await stored()).map((template) => template.name)).toEqual([
      'Status',
      'Owner',
      'Number',
    ]);
  });

  it('numbers a name that is taken', async () => {
    const { board, view } = await open();
    await addProperty(board, view, 'text', 'Text');
    await addProperty(board, view, 'text', 'Text');
    expect(board.board?.cardProperties.map((template) => template.name).slice(-2)).toEqual([
      'Text',
      'Text 2',
    ]);
  });

  it('shows the new property as a column of the table it was added from, and nowhere else', async () => {
    const { board, view } = await open();
    await board.setViewFields(view.id, { viewType: 'table' });
    const table = board.view(view.id);
    const id = await addProperty(board, table, 'text', 'Text');
    expect(board.view(view.id)?.fields.shownProperties).toEqual([id]);

    await board.setViewFields(view.id, { viewType: 'board' });
    const other = await addProperty(board, board.view(view.id), 'text', 'Text');
    expect(board.view(view.id)?.fields.shownProperties).toEqual([id]);
    expect(other).toBeDefined();
  });

  it('is undone as one step, the column with the property', async () => {
    const { board, workspace, view, stored } = await open();
    await board.setViewFields(view.id, { viewType: 'table' });
    await addProperty(board, board.view(view.id), 'text', 'Text');
    await workspace.undoLast();
    expect(board.board?.cardProperties).toHaveLength(2);
    expect(board.view(view.id)?.fields.shownProperties).toEqual([]);
    expect(await stored()).toHaveLength(2);
  });

  it('answers with nothing when the board refuses it', async () => {
    const { board, backend, workspace, view } = await open();
    backend.failNext('patchContainer', new BackendError(403));
    expect(await addProperty(board, view, 'text', 'Text')).toBeUndefined();
    expect(board.board?.cardProperties).toHaveLength(2);
    expect(workspace.problems.at(-1)?.action).toBe('save');
  });
});

describe('naming a property', () => {
  it('stores the new name, trimmed', async () => {
    const { board, stored, definition } = await open();
    await renameProperty(board, STATUS, '  Stage ');
    expect(definition(STATUS)?.name).toBe('Stage');
    expect((await stored())[0]?.name).toBe('Stage');
  });

  it('sends nothing for an empty name or the name it has', async () => {
    const { board, workspace, definition } = await open();
    await renameProperty(board, STATUS, '   ');
    await renameProperty(board, STATUS, 'Status');
    expect(definition(STATUS)?.name).toBe('Status');
    expect(workspace.canUndo).toBe(false);
  });

  it('keeps the kind, the options and the values', async () => {
    const { board, definition, cardNamed } = await open();
    await renameProperty(board, STATUS, 'Stage');
    expect(definition(STATUS)).toMatchObject({ type: 'select' });
    expect(definition(STATUS)?.options).toHaveLength(3);
    expect(cardNamed('Alpha').fields.properties[STATUS]).toBe('todo');
  });
});

describe('changing the kind of a property', () => {
  it('rewrites the definition and the values, and one undo takes all of it back', async () => {
    const { board, backend, workspace, definition, cardNamed } = await open();
    const template = definition(STATUS);
    if (!template) throw new Error('no status');
    const plan = planTypeChange(template, 'text', board.cards);
    expect(plan).toMatchObject({ kept: 3, lost: 0 });

    await changePropertyType(board, plan);
    expect(definition(STATUS)).toMatchObject({ type: 'text', options: [] });
    expect(cardNamed('Alpha').fields.properties[STATUS]).toBe('To do');
    expect(cardNamed('Gamma').fields.properties[STATUS]).toBe('Doing');
    const alpha = await backend.card(board.boardId, cardNamed('Alpha').id);
    expect(alpha.fields.properties[STATUS]).toBe('To do');

    await workspace.undoLast();
    expect(definition(STATUS)).toMatchObject({ type: 'select' });
    expect(definition(STATUS)?.options.map((option) => option.id)).toEqual([
      'todo',
      'doing',
      'done',
    ]);
    expect(cardNamed('Alpha').fields.properties[STATUS]).toBe('todo');
    expect(cardNamed('Gamma').fields.properties[STATUS]).toBe('doing');
    expect(workspace.canUndo).toBe(false);
  });

  it('removes the values the new kind cannot read', async () => {
    const { board, definition, cardNamed } = await open();
    const template = definition(STATUS);
    if (!template) throw new Error('no status');
    const plan = planTypeChange(template, 'date', board.cards);
    expect(plan).toMatchObject({ kept: 0, lost: 3 });
    await changePropertyType(board, plan);
    expect(definition(STATUS)?.type).toBe('date');
    expect(STATUS in cardNamed('Alpha').fields.properties).toBe(false);
  });

  it('removes the values first, so that one undo brings them back under the old kind', async () => {
    const { board, backend, workspace, definition, cardNamed } = await open();
    const template = definition(STATUS);
    if (!template) throw new Error('no status');
    for (const kind of ['relation', 'date'] as const) {
      await changePropertyType(board, planTypeChange(template, kind, board.cards));
      expect(definition(STATUS)?.type).toBe(kind);
      expect(STATUS in cardNamed('Alpha').fields.properties).toBe(false);
      await workspace.undoLast();
      expect(definition(STATUS)?.type).toBe('select');
      const alpha = await backend.card(board.boardId, cardNamed('Alpha').id);
      expect(alpha.fields.properties[STATUS]).toBe('todo');
      expect(workspace.canUndo).toBe(false);
    }
  });

  it('keeps a value left over from a kind before when the change is refused, and undoes it', async () => {
    const { board, backend, workspace, definition } = await open();
    // Turned into a card relation earlier: the values of the select stay, as the server leaves them.
    await board.changeBoard((current) => ({
      ...current,
      cardProperties: current.cardProperties.map((property) =>
        property.id === STATUS ? { ...property, type: 'relation', options: [] } : property,
      ),
    }));
    const template = definition(STATUS);
    const source = board.cards[0];
    if (!template || !source) throw new Error('a property and a card are needed');
    const stored = async () =>
      (await backend.card(board.boardId, source.id)).fields.properties[STATUS];
    const before = await stored();
    expect(before).toBeDefined();

    backend.failNext('patchContainer', new BackendError(403));
    await changePropertyType(board, planTypeChange(template, 'text', board.cards));
    expect(
      (await backend.container(board.boardId)).cardProperties.find((p) => p.id === STATUS)?.type,
    ).toBe('relation');
    expect(await stored()).toEqual(before);

    await changePropertyType(board, planTypeChange(template, 'text', board.cards));
    expect(await stored()).toBeUndefined();
    await workspace.undoLast();
    expect(definition(STATUS)?.type).toBe('relation');
    expect(await stored()).toEqual(before);
  });

  it('leaves a value no kind of the change takes, so that undo is not stopped by it', async () => {
    const { board, backend, workspace, definition } = await open();
    await board.changeBoard((current) => ({
      ...current,
      cardProperties: current.cardProperties.map((property) =>
        property.id === STATUS ? { ...property, type: 'relation', options: [] } : property,
      ),
    }));
    const template = definition(STATUS);
    const source = board.cards[0];
    if (!template || !source) throw new Error('a property and a card are needed');
    const stored = async () =>
      (await backend.card(board.boardId, source.id)).fields.properties[STATUS];
    const before = await stored();
    expect(typeof before).toBe('string');

    // A select value is no relation value and no date value: it stays where it is.
    await changePropertyType(board, planTypeChange(template, 'date', board.cards));
    expect(definition(STATUS)?.type).toBe('date');
    expect(await stored()).toEqual(before);
    await workspace.undoLast();
    expect(workspace.problems).toEqual([]);
    expect(definition(STATUS)?.type).toBe('relation');
    expect(await stored()).toEqual(before);
  });

  it('takes the views off what they wrote for the old kind', async () => {
    const { board, view, definition } = await open();
    expect(view.fields.groupBy).toBe(STATUS);
    const template = definition(STATUS);
    if (!template) throw new Error('no status');
    await changePropertyType(board, planTypeChange(template, 'text', board.cards));
    const after = board.view(view.id);
    expect(after?.fields.groupBy).toBeUndefined();
    expect(after?.fields.visibleGroups).toEqual([]);
  });

  it('leaves views and values alone when the board refuses the change', async () => {
    const { board, backend, view, definition, cardNamed } = await open();
    const template = definition(STATUS);
    if (!template) throw new Error('no status');
    backend.failNext('patchContainer', new BackendError(403));
    await changePropertyType(board, planTypeChange(template, 'text', board.cards));
    expect(definition(STATUS)?.type).toBe('select');
    expect(board.view(view.id)?.fields.groupBy).toBe(STATUS);
    expect(cardNamed('Alpha').fields.properties[STATUS]).toBe('todo');

    backend.failNext('patchContainer', new BackendError(403));
    await changePropertyType(board, planTypeChange(template, 'relation', board.cards));
    expect(definition(STATUS)?.type).toBe('select');
    const alpha = await backend.card(board.boardId, cardNamed('Alpha').id);
    expect(alpha.fields.properties[STATUS]).toBe('todo');
  });
});

describe('deleting a property', () => {
  it('removes the definition and what the views said about it', async () => {
    const { board, view, stored, backend } = await open();
    await board.setViewFields(view.id, {
      shownProperties: [STATUS, OWNER],
      sort: [{ propertyId: STATUS, reversed: false }],
      filter: {
        operation: 'and',
        filters: [{ propertyId: STATUS, condition: 'includes', values: ['todo'] }],
      },
    });
    await deleteProperty(board, STATUS);
    expect((await stored()).map((template) => template.id)).toEqual([OWNER]);
    const fields = board.view(view.id)?.fields;
    expect(fields?.shownProperties).toEqual([OWNER]);
    expect(fields?.sort).toEqual([]);
    expect(fields?.filter.filters).toEqual([]);
    expect(fields?.groupBy).toBeUndefined();
    const storedView = (await backend.views(board.boardId))[0];
    expect(storedView?.fields.groupBy).toBeUndefined();
    expect(storedView?.fields.shownProperties).toEqual([OWNER]);
  });

  it('comes back with its values and its place in the views on one undo', async () => {
    const { board, workspace, view, definition, cardNamed } = await open();
    await deleteProperty(board, STATUS);
    expect(definition(STATUS)).toBeUndefined();
    await workspace.undoLast();
    await settle();
    expect(definition(STATUS)?.options).toHaveLength(3);
    expect(board.view(view.id)?.fields.groupBy).toBe(STATUS);
    expect(board.view(view.id)?.fields.visibleGroups).toEqual(['todo', 'doing', 'done', '']);
    expect(cardNamed('Alpha').fields.properties[STATUS]).toBe('todo');
  });

  it('keeps the views when the board refuses', async () => {
    const { board, backend, view, definition } = await open();
    backend.failNext('patchContainer', new BackendError(403));
    await deleteProperty(board, STATUS);
    expect(definition(STATUS)).toBeDefined();
    expect(board.view(view.id)?.fields.groupBy).toBe(STATUS);
  });
});

describe('a card relation a Gantt view reads as its dependencies', () => {
  async function withRelation() {
    const made = await open();
    const { board, view } = made;
    const relation = (await addProperty(board, view, 'relation', 'After')) ?? '';
    await board.setViewFields(view.id, { viewType: 'gantt', dependencyPropertyId: relation });
    const named = async (): Promise<string | undefined> => {
      const kept = (await made.backend.views(board.boardId))[0]?.fields.dependencyPropertyId;
      expect(board.view(view.id)?.fields.dependencyPropertyId).toBe(kept);
      return kept;
    };
    return { ...made, relation, named };
  }

  it('is no longer read once deleted, and is read again once an undo brings it back', async () => {
    const { board, workspace, relation, definition, named } = await withRelation();
    await deleteProperty(board, relation);
    expect(await named()).toBeUndefined();
    await workspace.undoLast();
    expect(definition(relation)?.type).toBe('relation');
    expect(await named()).toBe(relation);
  });

  it('is no longer read once it is another kind, and is read again once that is undone', async () => {
    const { board, workspace, relation, definition, named } = await withRelation();
    const template = definition(relation);
    if (!template) throw new Error('no relation');
    await changePropertyType(board, planTypeChange(template, 'text', board.cards));
    expect(definition(relation)?.type).toBe('text');
    expect(await named()).toBeUndefined();
    await workspace.undoLast();
    expect(definition(relation)?.type).toBe('relation');
    expect(await named()).toBe(relation);
  });
});

describe('the options of a select, as its editor reports them', () => {
  it('makes an option and hands it back for the value', async () => {
    const { board, stored, definition } = await open();
    const option = await optionCallbacks(board, STATUS).onCreateOption('Blocked');
    expect(option).toMatchObject({ label: 'Blocked', color: 'default' });
    expect(definition(STATUS)?.options.at(-1)).toEqual(option);
    expect((await stored())[0]?.options.at(-1)).toEqual(option);
  });

  it('refuses to hand back an option the board did not take', async () => {
    const { board, backend } = await open();
    backend.failNext('patchContainer', new BackendError(403));
    await expect(optionCallbacks(board, STATUS).onCreateOption('Blocked')).rejects.toThrow();
  });

  it('renames, recolours and deletes an option', async () => {
    const { board, definition, workspace } = await open();
    const callbacks = optionCallbacks(board, STATUS);
    const todo = definition(STATUS)?.options[0];
    if (!todo) throw new Error('no option');
    callbacks.onRenameOption(todo, 'Next');
    callbacks.onRecolorOption(todo, 'red');
    await settle();
    expect(definition(STATUS)?.options[0]).toEqual({
      id: 'todo',
      label: 'Next',
      color: 'red',
    });
    callbacks.onDeleteOption(todo);
    await settle();
    expect(definition(STATUS)?.options.map((option) => option.id)).toEqual(['doing', 'done']);
    await workspace.undoLast();
    expect(definition(STATUS)?.options.map((option) => option.id)).toContain('todo');
  });
});
