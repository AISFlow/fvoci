/**
 * Initial full-document and subsequent changed-range schema checks for
 * collaborative bodies (`content-check.ts` and `BodyEditor.svelte`).
 */
import { Editor } from '@tiptap/core';
import { Fragment, Node as ProseMirrorNode, Schema, Slice } from '@tiptap/pm/model';
import { EditorState, Selection, type Transaction } from '@tiptap/pm/state';
import {
  AddMarkStep,
  AddNodeMarkStep,
  AttrStep,
  DocAttrStep,
  Mapping,
  ReplaceAroundStep,
  ReplaceStep,
  Step,
  StepMap,
  StepResult,
} from '@tiptap/pm/transform';
import { ySyncPluginKey } from '@tiptap/y-tiptap';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import {
  checkChangedContent,
  CheckedCollaboration,
  contentCheckPlugin,
  hasInvalidContent,
} from '$lib/editor/content-check.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { mayWrite } from '$lib/editor/shared-place.ts';

const editors: Editor[] = [];

afterEach(() => {
  vi.restoreAllMocks();
  for (const editor of editors.splice(0)) editor.destroy();
});

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** Runs `mutate` on a peer copy of `shared` and applies the delta back as a remote update. */
function fromRoom(shared: Y.Doc, mutate: (fragment: Y.XmlFragment) => void): void {
  const other = new Y.Doc();
  Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
  const before = Y.encodeStateVector(other);
  mutate(other.getXmlFragment(BODY_FIELD));
  Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, before), 'room');
  other.destroy();
}

function xmlParagraph(value: string, attrs?: Record<string, unknown>): Y.XmlElement {
  const element = new Y.XmlElement('paragraph');
  const text = new Y.XmlText();
  if (value.length > 0) text.insert(0, value, attrs);
  element.insert(0, [text]);
  return element;
}

function typeAtEnd(editor: Editor, value: string): void {
  const end = Selection.atEnd(editor.state.doc);
  editor.view.dispatch(editor.state.tr.setSelection(end).insertText(value, end.from));
}

/** A custom step with an empty StepMap that replaces the doc directly, for fallback testing. */
class CustomDocStep extends Step {
  readonly nextDoc: ProseMirrorNode;

  constructor(nextDoc: ProseMirrorNode) {
    super();
    this.nextDoc = nextDoc;
  }

  apply(): StepResult {
    return StepResult.ok(this.nextDoc);
  }

  override getMap(): StepMap {
    return StepMap.empty;
  }

  invert(doc: ProseMirrorNode): Step {
    return new CustomDocStep(doc);
  }

  map(): Step | null {
    return null;
  }

  toJSON(): Record<string, unknown> {
    return { stepType: 'customDoc' };
  }
}

describe('CheckedCollaboration incremental check scope', () => {
  it('checks the whole document once on initial sync and only touched blocks on remote updates', () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      const blocks = Array.from({ length: 20 }, (_, i) => xmlParagraph(`line ${i}`));
      fragment.insert(0, blocks);
    });

    const checked: string[] = [];
    const descriptor = Object.getOwnPropertyDescriptor(ProseMirrorNode.prototype, 'check');
    const originalCheck = descriptor?.value as (this: ProseMirrorNode) => void;
    vi.spyOn(ProseMirrorNode.prototype, 'check').mockImplementation(function (
      this: ProseMirrorNode,
    ) {
      checked.push(`${this.type.name}:${this.textContent.slice(0, 12)}`);
      originalCheck.call(this);
    });

    const onContentError = vi.fn();
    const editor = new Editor({
      element: document.createElement('div'),
      extensions: [
        CheckedCollaboration.configure({ document: shared, field: BODY_FIELD }),
        ...bodyExtensions({ history: false }),
      ],
      onContentError,
    });
    editors.push(editor);

    // Initial sync ran a full `doc.check()` covering the root doc and all 20 paragraphs.
    expect(checked[0]).toMatch(/^doc:/);
    expect(checked.filter((entry) => entry.startsWith('paragraph:'))).toHaveLength(20);
    expect(onContentError).not.toHaveBeenCalled();

    checked.length = 0;

    // A remote update touching only the last paragraph checks only that paragraph, not the other 19.
    fromRoom(shared, (fragment) => {
      const last = fragment.get(fragment.length - 1) as Y.XmlElement;
      const text = last.get(0) as Y.XmlText;
      text.insert(text.length, '!');
    });

    expect(checked.some((entry) => entry.startsWith('doc:'))).toBe(false);
    expect(checked.filter((entry) => entry.startsWith('paragraph:'))).toEqual([
      'paragraph:line 19!',
    ]);
    expect(editor.state.doc.lastChild?.textContent).toBe('line 19!');
    expect(onContentError).not.toHaveBeenCalled();
  });

  it('supports disableCollaboration when the error callback disables syncing', () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      fragment.insert(0, [xmlParagraph('valid')]);
    });

    const onContentError = vi.fn(
      ({ disableCollaboration }: { disableCollaboration: () => void }) => {
        disableCollaboration();
      },
    );
    const editor = new Editor({
      element: document.createElement('div'),
      extensions: [
        CheckedCollaboration.configure({ document: shared, field: BODY_FIELD }),
        ...bodyExtensions({ history: false }),
      ],
      onContentError,
    });
    editors.push(editor);

    // Trigger a ProseMirror mark-exclusion error (`code` excludes `bold`).
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
      text.format(0, 5, { code: {}, bold: {} });
    });

    expect(onContentError).toHaveBeenCalledTimes(1);
    expect(editor.storage.collaboration.isDisabled).toBe(true);
  });
});

describe('BodyEditor collaborative schema protection', () => {
  it('catches invalid excluded marks on initial load, keeps Y.Doc intact, and recovers when mended', async () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      // `code` excludes all other marks in schema; y-tiptap projects both onto the text node.
      fragment.insert(0, [xmlParagraph('conflict', { code: {}, bold: {} })]);
    });

    const onFitsAgain = vi.fn();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared,
      canWrite: true,
      onFitsAgain,
    });
    const editor = editorIn(container);

    await vi.waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(mayWrite(editor.view)).toBe(false);
    expect(container.querySelector('.doc-editor-status')?.textContent.trim()).not.toBe('');
    expect(onFitsAgain).not.toHaveBeenCalled();

    // Local edits are blocked and the shared Y.Doc is untouched.
    typeAtEnd(editor, 'blocked');
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).toContain('conflict');
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).not.toContain('blocked');

    // Removing the conflicting mark makes the body fit again.
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
      text.format(0, text.length, { code: null });
    });

    await vi.waitFor(() => {
      expect(onFitsAgain).toHaveBeenCalled();
    });
    expect(hasInvalidContent(editor.state)).toBe(false);
  });

  it('catches remote excluded marks incrementally, stays blocked on unrelated edits, and recovers when fixed', async () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      fragment.insert(0, [xmlParagraph('first'), xmlParagraph('second')]);
    });

    const onFitsAgain = vi.fn();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared,
      canWrite: true,
      onFitsAgain,
    });
    const editor = editorIn(container);
    expect(editor.isEditable).toBe(true);

    // Record a local edit so undo stack is non-empty, then verify undo is blocked once invalid.
    typeAtEnd(editor, '!');
    expect(editor.state.doc.lastChild?.textContent).toBe('second!');

    // Remote update introduces conflicting marks in paragraph 0.
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
      text.format(0, 5, { code: {}, italic: {} });
    });

    await vi.waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(mayWrite(editor.view)).toBe(false);
    expect(onFitsAgain).not.toHaveBeenCalled();

    // Shared undo/redo commands are blocked while invalid so nothing writes back to Y.Doc.
    expect(editor.commands.undo()).toBe(false);
    expect(editor.commands.redo()).toBe(false);

    // An unrelated edit to paragraph 1 must not clear the error or call onFitsAgain.
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(1) as Y.XmlElement).get(0) as Y.XmlText;
      text.insert(text.length, ' more');
    });
    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(onFitsAgain).not.toHaveBeenCalled();

    // Local edits remain blocked and nothing is deleted from the shared Y.Doc.
    typeAtEnd(editor, ' nope');
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).not.toContain('nope');
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).toContain('second! more');

    // Fixing the conflicting mark in paragraph 0 restores validity.
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
      text.format(0, 5, { code: null });
    });

    await vi.waitFor(() => {
      expect(onFitsAgain).toHaveBeenCalled();
    });
  });

  it('catches remote unknown marks, invalid nested children, invalid top-level nodes, and empty shared docs', async () => {
    const cases: {
      name: string;
      corrupt: (fragment: Y.XmlFragment) => void;
      repair: (fragment: Y.XmlFragment) => void;
    }[] = [
      {
        name: 'unknown mark',
        corrupt: (fragment) => {
          const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
          text.format(0, 4, { futureMark: {} });
        },
        repair: (fragment) => {
          const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
          text.format(0, 4, { futureMark: null });
        },
      },
      {
        name: 'disallowed child inside paragraph',
        corrupt: (fragment) => {
          const para = fragment.get(0) as Y.XmlElement;
          const quote = new Y.XmlElement('blockquote');
          quote.insert(0, [xmlParagraph('nested')]);
          para.insert(para.length, [quote]);
        },
        repair: (fragment) => {
          const para = fragment.get(0) as Y.XmlElement;
          para.delete(para.length - 1, 1);
        },
      },
      {
        name: 'disallowed top-level node',
        corrupt: (fragment) => {
          const item = new Y.XmlElement('listItem');
          item.insert(0, [xmlParagraph('stray')]);
          fragment.insert(fragment.length, [item]);
        },
        repair: (fragment) => {
          fragment.delete(fragment.length - 1, 1);
        },
      },
      {
        name: 'empty shared document after content existed',
        corrupt: (fragment) => {
          fragment.delete(0, fragment.length);
        },
        repair: (fragment) => {
          fragment.insert(0, [xmlParagraph('restored')]);
        },
      },
    ];

    for (const scenario of cases) {
      const shared = new Y.Doc();
      fromRoom(shared, (fragment) => {
        fragment.insert(0, [xmlParagraph('good')]);
      });
      const onFitsAgain = vi.fn();
      const { container, unmount } = render(BodyEditor, {
        doc: null,
        collab: shared,
        onFitsAgain,
      });
      const editor = editorIn(container);

      fromRoom(shared, scenario.corrupt);
      await vi.waitFor(() => {
        expect(editor.isEditable, scenario.name).toBe(false);
      });
      expect(hasInvalidContent(editor.state), scenario.name).toBe(true);
      expect(onFitsAgain, scenario.name).not.toHaveBeenCalled();

      fromRoom(shared, scenario.repair);
      await vi.waitFor(() => {
        expect(onFitsAgain, scenario.name).toHaveBeenCalled();
      });
      unmount();
    }
  });

  it('re-checks the full document when a ySyncPlugin error clears while another schema error remains', async () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      fragment.insert(0, [
        xmlParagraph('bad marks', { code: {}, bold: {} }),
        new Y.XmlElement('futureNode'),
      ]);
    });

    const onFitsAgain = vi.fn();
    const { container } = render(BodyEditor, { doc: null, collab: shared, onFitsAgain });
    const editor = editorIn(container);

    await vi.waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });

    // Removing only `futureNode` clears ySyncPlugin's projection error, but `[code, bold]` remains!
    fromRoom(shared, (fragment) => {
      fragment.delete(1, 1);
    });
    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(onFitsAgain).not.toHaveBeenCalled();

    // Now removing `code` makes the whole document valid.
    fromRoom(shared, (fragment) => {
      const text = (fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText;
      text.format(0, text.length, { code: null });
    });
    await vi.waitFor(() => {
      expect(onFitsAgain).toHaveBeenCalled();
    });
  });
});

describe('checkChangedContent ProseMirror edge cases', () => {
  let strictValidation = false;

  const validatedSchema = new Schema({
    nodes: {
      doc: {
        content: 'block+',
        attrs: {
          version: {
            default: 1,
            validate: (value: unknown) => {
              if (strictValidation && (typeof value !== 'number' || value < 1)) {
                throw new RangeError('invalid doc version');
              }
            },
          },
        },
      },
      paragraph: {
        group: 'block',
        content: 'inline*',
        marks: '_',
        attrs: {
          align: {
            default: 'left',
            validate: (value: unknown) => {
              if (strictValidation && value !== 'left' && value !== 'right') {
                throw new RangeError('invalid align');
              }
            },
          },
        },
      },
      item: {
        content: 'paragraph+',
      },
      chip: {
        group: 'inline',
        inline: true,
        atom: true,
      },
      text: { group: 'inline' },
    },
    marks: {
      tag: {
        attrs: {
          label: {
            default: 'ok',
            validate: (value: unknown) => {
              if (strictValidation && value === 'bad') throw new RangeError('invalid tag');
            },
          },
        },
      },
      nodeTag: {
        attrs: {
          label: {
            default: 'ok',
            validate: (value: unknown) => {
              if (strictValidation && value === 'bad') throw new RangeError('invalid nodeTag');
            },
          },
        },
      },
    },
  });

  afterEach(() => {
    strictValidation = false;
  });

  it('catches invalid attrs and marks on steps whose StepMap is empty', () => {
    strictValidation = false;
    const baseDoc = validatedSchema.node('doc', null, [
      validatedSchema.node('paragraph', null, [
        validatedSchema.text('hello'),
        validatedSchema.node('chip'),
      ]),
      validatedSchema.node('paragraph', null, [validatedSchema.text('world')]),
    ]);
    const state = EditorState.create({ doc: baseDoc });

    // Build transactions while lenient, then enforce strict validation so `checkChangedContent`
    // must locate and check the touched node even though every step has `StepMap.empty`.
    const badMarkTr = state.tr.step(
      new AddMarkStep(1, 6, validatedSchema.mark('tag', { label: 'bad' })),
    );
    const badAttrTr = state.tr.step(new AttrStep(0, 'align', 'center'));
    const badDocAttrTr = state.tr.step(new DocAttrStep('version', 0));
    // Construct a doc whose own attrs carry a prototype-property key (`toString`) directly,
    // since `DocAttrStep.apply` strips undeclared keys via `computeAttrs`.
    const docWithProtoAttr = Object.assign(
      Object.create(Object.getPrototypeOf(baseDoc) as object),
      baseDoc,
      { attrs: { ...baseDoc.attrs, toString: 'x' } },
    ) as ProseMirrorNode;
    const protoDocAttrTr = {
      before: baseDoc,
      doc: docWithProtoAttr,
      steps: [new DocAttrStep('toString', 'x')],
      mapping: new Mapping([StepMap.empty]),
    } as unknown as Transaction;
    const docWithUndefinedAttr = Object.assign(
      Object.create(Object.getPrototypeOf(baseDoc) as object),
      baseDoc,
      { attrs: { version: undefined } },
    ) as ProseMirrorNode;
    const undefinedDocAttrTr = {
      before: baseDoc,
      doc: docWithUndefinedAttr,
      steps: [new DocAttrStep('version', undefined)],
      mapping: new Mapping([StepMap.empty]),
    } as unknown as Transaction;
    // Inline atom `chip` sits at position 6 inside paragraph 0.
    const badNodeMarkTr = state.tr.step(
      new AddNodeMarkStep(6, validatedSchema.mark('nodeTag', { label: 'bad' })),
    );

    strictValidation = true;
    expect(() => {
      checkChangedContent(badMarkTr);
    }).toThrow('invalid tag');
    expect(() => {
      checkChangedContent(badAttrTr);
    }).toThrow('invalid align');
    expect(() => {
      checkChangedContent(badDocAttrTr);
    }).toThrow('invalid doc version');
    expect(() => {
      checkChangedContent(protoDocAttrTr);
    }).toThrow('Unsupported attribute toString');
    expect(() => {
      checkChangedContent(undefinedDocAttrTr);
    }).toThrow('invalid doc version');
    expect(() => {
      checkChangedContent(badNodeMarkTr);
    }).toThrow('invalid nodeTag');
  });

  it('tracks multi-step mappings and ReplaceAroundStep unwraps into changed top-level blocks', () => {
    const wrapSchema = new Schema({
      nodes: {
        doc: { content: 'block+' },
        quote: { group: 'block', content: 'paragraph+' },
        paragraph: {
          group: 'block',
          content: 'text*',
          attrs: {
            align: {
              default: 'left',
              validate: (value: unknown) => {
                if (strictValidation && value !== 'left') throw new RangeError('invalid align');
              },
            },
          },
        },
        text: { group: 'inline' },
      },
    });

    strictValidation = false;
    const wrappedDoc = wrapSchema.node('doc', null, [
      wrapSchema.node('quote', null, [
        wrapSchema.node('paragraph', { align: 'right' }, [wrapSchema.text('inside')]),
      ]),
      wrapSchema.node('paragraph', null, [wrapSchema.text('untouched')]),
    ]);
    const state = EditorState.create({ doc: wrappedDoc });
    const quoteSize = wrappedDoc.child(0).nodeSize;
    // Unwrap `quote(paragraph)` to `paragraph` using ReplaceAroundStep.
    const unwrapTr = state.tr.step(
      new ReplaceAroundStep(0, quoteSize, 1, quoteSize - 1, Slice.empty, 0),
    );

    // Also test AttrStep followed by a preceding ReplaceStep shifting positions forward.
    const shiftedTr = state.tr
      .step(new AttrStep(quoteSize, 'align', 'right'))
      .insertText('prefix ', 2);

    strictValidation = true;
    expect(() => {
      checkChangedContent(unwrapTr);
    }).toThrow('invalid align');
    expect(() => {
      checkChangedContent(shiftedTr);
    }).toThrow('invalid align');
  });

  it('does not clear invalid content on metadata-only ySyncPluginKey snapshot transactions', () => {
    const shared = new Y.Doc();
    fromRoom(shared, (fragment) => {
      fragment.insert(0, [xmlParagraph('conflict', { code: {}, bold: {} })]);
    });

    const onContentError = vi.fn();
    const editor = new Editor({
      element: document.createElement('div'),
      extensions: [
        CheckedCollaboration.configure({ document: shared, field: BODY_FIELD }),
        ...bodyExtensions({ history: false }),
      ],
      onContentError,
    });
    editors.push(editor);

    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(onContentError).toHaveBeenCalledTimes(1);

    // Metadata-only snapshot request must NOT validate the stale doc or clear the error.
    editor.view.dispatch(
      editor.state.tr.setMeta(ySyncPluginKey, {
        snapshot: Y.snapshot(shared),
        prevSnapshot: Y.emptySnapshot,
      }),
    );
    expect(hasInvalidContent(editor.state)).toBe(true);
  });

  it('catches empty doc, disallowed top-level child, disallowed top-level mark, and non-uniform doc content', () => {
    // 1. Disallowed top-level child (`item` is not in group `block`) on uniform `block+`.
    const baseDoc = validatedSchema.node('doc', null, [
      validatedSchema.node('paragraph', null, [validatedSchema.text('ok')]),
    ]);
    const state = EditorState.create({ doc: baseDoc });
    const badTopDoc = validatedSchema.topNodeType.create(null, [
      validatedSchema.node('item', null, [validatedSchema.node('paragraph')]),
    ]);
    const badTopTr = state.tr.step(new CustomDocStep(badTopDoc));
    expect(() => {
      checkChangedContent(badTopTr);
    }).toThrow('Invalid content for node doc');

    // Also test the incremental (non-fallback) uniform-doc child check directly with a ReplaceStep.
    const badTopIncrementalTr = {
      before: baseDoc,
      doc: badTopDoc,
      steps: [new ReplaceStep(0, baseDoc.content.size, new Slice(badTopDoc.content, 0, 0))],
      mapping: new Mapping([new StepMap([0, baseDoc.content.size, badTopDoc.content.size])]),
    } as unknown as Transaction;
    expect(() => {
      checkChangedContent(badTopIncrementalTr);
    }).toThrow('Invalid content for node doc');

    // 1b. Disallowed mark on a top-level block child (`doc` allows no marks on direct children).
    const markedTopBlock = validatedSchema
      .node('paragraph', null, [validatedSchema.text('marked')])
      .mark([validatedSchema.mark('nodeTag', { label: 'ok' })]);
    const markedTopDoc = validatedSchema.topNodeType.create(null, [markedTopBlock]);
    const markedTopTr = {
      before: baseDoc,
      doc: markedTopDoc,
      steps: [new ReplaceStep(0, baseDoc.content.size, new Slice(markedTopDoc.content, 0, 0))],
      mapping: new Mapping([new StepMap([0, baseDoc.content.size, markedTopDoc.content.size])]),
    } as unknown as Transaction;
    expect(() => {
      checkChangedContent(markedTopTr);
    }).toThrow('Invalid content for node doc');

    // 2. Empty doc on `block+` (invalid end state), via incremental path (StepMap covering deletion).
    const emptyInvalidDoc = validatedSchema.topNodeType.create(null, Fragment.empty);
    const emptyIncrementalTr = {
      before: baseDoc,
      doc: emptyInvalidDoc,
      steps: [new ReplaceStep(0, baseDoc.content.size, Slice.empty)],
      mapping: new Mapping([new StepMap([0, baseDoc.content.size, 0])]),
    } as unknown as Transaction;
    expect(() => {
      checkChangedContent(emptyIncrementalTr);
    }).toThrow('Invalid content for node doc');

    // 3. Non-uniform doc content expression (`heading paragraph+`): missing leading heading fails.
    const titledSchema = new Schema({
      nodes: {
        doc: { content: 'heading paragraph+' },
        heading: { content: 'text*' },
        paragraph: { content: 'text*' },
        text: {},
      },
    });
    const titledDoc = titledSchema.node('doc', null, [
      titledSchema.node('heading', null, [titledSchema.text('Title')]),
      titledSchema.node('paragraph', null, [titledSchema.text('Body')]),
    ]);
    const secondPara = titledSchema.node('paragraph', null, [titledSchema.text('Not a heading')]);
    const brokenOrderDoc = titledSchema.topNodeType.create(null, [secondPara, titledDoc.child(1)]);
    const brokenOrderTr = {
      before: titledDoc,
      doc: brokenOrderDoc,
      steps: [
        new ReplaceStep(0, titledDoc.child(0).nodeSize, new Slice(Fragment.from(secondPara), 0, 0)),
      ],
      mapping: new Mapping([new StepMap([0, titledDoc.child(0).nodeSize, secondPara.nodeSize])]),
    } as unknown as Transaction;
    expect(() => {
      checkChangedContent(brokenOrderTr);
    }).toThrow('Invalid content for node doc');
  });

  it('standalone contentCheckPlugin catches errors on non-shared transactions when onlyShared is false', () => {
    strictValidation = false;
    const errors: Error[] = [];
    const plugin = contentCheckPlugin({
      onError: (error) => errors.push(error),
      onlyShared: false,
    });
    const baseDoc = validatedSchema.node('doc', null, [
      validatedSchema.node('paragraph', null, [validatedSchema.text('a')]),
      validatedSchema.node('paragraph', null, [validatedSchema.text('b')]),
    ]);
    let state = EditorState.create({ doc: baseDoc, plugins: [plugin] });

    // First transaction passes initial full check.
    state = state.apply(state.tr.insertText('!', 2));
    expect(errors).toEqual([]);
    expect(hasInvalidContent(state)).toBe(false);

    // Second transaction violates attr validation via incremental check and is rejected.
    const badTr = state.tr.step(new AttrStep(0, 'align', 'bogus'));
    strictValidation = true;
    const next = state.apply(badTr);
    expect(errors).toHaveLength(1);
    expect(errors[0]?.message).toContain('invalid align');
    expect(hasInvalidContent(next)).toBe(true);
    expect(next.doc).toBe(state.doc);
  });
});
