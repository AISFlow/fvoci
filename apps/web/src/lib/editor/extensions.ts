/**
 * The editor's building blocks, configured so that the editor can only
 * produce what schema.ts allows.
 *
 * Each extension is taken on its own instead of through a kit, so that the
 * list here can be read against the list there. Where an extension defines
 * attributes the stored document does not have (the target of a link, the
 * numbering style of a list), the extension is narrowed to the ones it has.
 */
import {
  Extension,
  mergeAttributes,
  Node,
  type AnyExtension,
  type Editor,
  type Range,
} from '@tiptap/core';
import { Blockquote } from '@tiptap/extension-blockquote';
import { Bold } from '@tiptap/extension-bold';
import { Code } from '@tiptap/extension-code';
import { HardBreak } from '@tiptap/extension-hard-break';
import { Heading } from '@tiptap/extension-heading';
import { HorizontalRule } from '@tiptap/extension-horizontal-rule';
import { Italic } from '@tiptap/extension-italic';
import { Link } from '@tiptap/extension-link';
import { BulletList, ListItem, ListKeymap, OrderedList, TaskList } from '@tiptap/extension-list';
import { Paragraph } from '@tiptap/extension-paragraph';
import { Strike } from '@tiptap/extension-strike';
import { Text } from '@tiptap/extension-text';
import { Dropcursor, Gapcursor, Placeholder, UndoRedo } from '@tiptap/extensions';
import { PluginKey } from '@tiptap/pm/state';
import {
  Suggestion,
  type SuggestionMatch,
  type SuggestionOptions,
  type Trigger,
} from '@tiptap/suggestion';

import { BodyCodeBlock } from './diagram-block.ts';
import { IMAGE_ATTRIBUTES, LINK_ATTRIBUTES } from './drawing.ts';
import { EditingPlace } from './editing-place.ts';
import { findEmojiTrigger } from './emoji-trigger.ts';
import { FileBlock } from './file-block.ts';
import { fileUrl } from './files.ts';
import { KeyReadsCaret } from './key-caret.ts';
import { BlockMath, InlineMath } from './math-nodes.ts';
import { Reference, type ReferenceOptions } from './reference.ts';
import { isAllowedHref, isFileId, isUserId } from './schema.ts';
import { BodyDocument, BodyTable, BodyTableCell, BodyTableHeader, BodyTableRow } from './table.ts';
import { BodyTaskItem } from './task-item.ts';

/** A link keeps its target and nothing else; how it opens is the app's business, not the document's. */
const BodyLink = Link.extend({
  addAttributes() {
    return {
      href: {
        default: null,
        parseHTML: (element: HTMLElement) => element.getAttribute('href')?.trim() ?? null,
      },
    };
  },
}).configure({
  openOnClick: false,
  autolink: true,
  linkOnPaste: true,
  defaultProtocol: 'https',
  protocols: ['mailto'],
  isAllowedUri: (url) => isAllowedHref(url),
  HTMLAttributes: { ...LINK_ATTRIBUTES },
});

/** A numbered list keeps the number it starts at. */
const BodyOrderedList = OrderedList.extend({
  addAttributes() {
    return {
      start: {
        default: 1,
        parseHTML: (element: HTMLElement) => {
          const start = Number.parseInt(element.getAttribute('start') ?? '', 10);
          return Number.isInteger(start) && start >= 0 && start <= 1_000_000_000 ? start : 1;
        },
      },
    };
  },
});

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    fileImage: {
      /** Inserts an image that shows an uploaded file. */
      setFileImage: (attrs: { fileId: string; alt?: string }) => ReturnType;
    };
  }
}

/**
 * An image is an uploaded file, named by its identifier. The address is made
 * when the node is drawn (files.ts) and is not part of the document, so an
 * `<img>` pasted from elsewhere, which has no file identifier, is not an image
 * here and is dropped.
 */
export const FileImage = Node.create({
  name: 'image',
  group: 'block',
  atom: true,
  draggable: true,

  addAttributes() {
    return {
      fileId: {
        default: null,
        parseHTML: (element: HTMLElement) => element.getAttribute('data-file-id'),
        renderHTML: (attrs: { fileId?: string | null }) => ({ 'data-file-id': attrs.fileId }),
      },
      alt: {
        default: null,
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: 'img[data-file-id]',
        getAttrs: (element: HTMLElement) =>
          isFileId(element.getAttribute('data-file-id')) ? null : false,
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    const fileId = typeof node.attrs.fileId === 'string' ? node.attrs.fileId : '';
    const src = fileId ? fileUrl(fileId) : undefined;
    return ['img', mergeAttributes(HTMLAttributes, src ? { src } : {}, { ...IMAGE_ATTRIBUTES })];
  },

  addCommands() {
    return {
      setFileImage:
        (attrs) =>
        ({ commands }) =>
          commands.insertContent({ type: this.name, attrs }),
    };
  },
});

/** One entry of an autocomplete list. */
export interface SuggestionEntry {
  /** What is inserted: a user identifier, or the emoji itself. */
  value: string;
  /** What the list shows. */
  label: string;
}

/**
 * Draws the list of an autocomplete and takes its keys. The editor does not
 * know how a list looks; the caller supplies this.
 */
export type SuggestionRenderer = SuggestionOptions<SuggestionEntry, SuggestionEntry>['render'];

export interface MentionOptions {
  /** The users whose name matches what was typed after `@`. */
  search: (query: string) => SuggestionEntry[] | Promise<SuggestionEntry[]>;
  /** The name shown for a user. */
  labelOf: (userId: string) => string;
  render?: SuggestionRenderer;
}

/**
 * A mention is the identifier of a user. The name is looked up when the node
 * is drawn, so a renamed user shows under the new name everywhere.
 */
export const Mention = Node.create<MentionOptions>({
  name: 'mention',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: false,

  addOptions() {
    return { search: () => [], labelOf: (userId) => userId };
  },

  addAttributes() {
    return {
      userId: {
        default: null,
        parseHTML: (element: HTMLElement) => element.getAttribute('data-user-id'),
        renderHTML: (attrs: { userId?: string | null }) => ({ 'data-user-id': attrs.userId }),
      },
    };
  },

  parseHTML() {
    // A mention that names no user (pasted from elsewhere, or from a view an
    // anonymous reader got) is not one: the server would refuse the body.
    return [
      {
        tag: 'span[data-mention][data-user-id]',
        getAttrs: (element: HTMLElement) =>
          isUserId(element.getAttribute('data-user-id')) ? null : false,
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    const label = this.options.labelOf(String(node.attrs.userId));
    return [
      'span',
      mergeAttributes(HTMLAttributes, { 'data-mention': '', class: 'mention' }),
      `@${label}`,
    ];
  },

  renderText({ node }) {
    return `@${this.options.labelOf(String(node.attrs.userId))}`;
  },

  addProseMirrorPlugins() {
    return [
      Suggestion<SuggestionEntry, SuggestionEntry>({
        editor: this.editor,
        pluginKey: new PluginKey('mentionSuggestion'),
        char: '@',
        items: ({ query }) => this.options.search(query),
        command: ({ editor, range, props }) => {
          insertAt(editor, range, [
            { type: this.name, attrs: { userId: props.value } },
            { type: 'text', text: ' ' },
          ]);
        },
        ...(this.options.render ? { render: this.options.render } : {}),
      }),
    ];
  },
});

export interface EmojiOptions {
  /** The emoji whose name matches what was typed after `:`. */
  search: (query: string) => SuggestionEntry[] | Promise<SuggestionEntry[]>;
  render?: SuggestionRenderer;
}

/**
 * The name of an emoji being typed at the cursor, if there is one. The rule
 * for the colon is in emoji-trigger.ts; this hands it the text of the line
 * up to the cursor. A node in the line that is not text (a mention, a
 * formula) counts as one character that is no letter.
 */
function emojiMatch({ $position }: Trigger): SuggestionMatch {
  if ($position.parent.type.spec.code) return null;
  const before = $position.parent.textBetween(0, $position.parentOffset, undefined, '￼');
  const trigger = findEmojiTrigger(before);
  if (!trigger) return null;
  return {
    range: { from: $position.start() + trigger.index, to: $position.pos },
    query: trigger.query,
    text: `:${trigger.query}`,
  };
}

/** Typing `:name` offers emoji. What is inserted is the character itself: an emoji is text. */
export const EmojiSuggestion = Extension.create<EmojiOptions>({
  name: 'emojiSuggestion',

  addOptions() {
    return { search: () => [] };
  },

  addProseMirrorPlugins() {
    return [
      Suggestion<SuggestionEntry, SuggestionEntry>({
        editor: this.editor,
        pluginKey: new PluginKey('emojiSuggestion'),
        char: ':',
        // `10:30` and `http://` are not the start of an emoji name; `안녕:` may be.
        findSuggestionMatch: emojiMatch,
        items: ({ query }) => (query.length === 0 ? [] : this.options.search(query)),
        command: ({ editor, range, props }) => {
          insertAt(editor, range, [{ type: 'text', text: props.value }]);
        },
        ...(this.options.render ? { render: this.options.render } : {}),
      }),
    ];
  },
});

function insertAt(
  editor: Editor,
  range: Range,
  content: { type: string; attrs?: object; text?: string }[],
): void {
  editor.chain().focus().insertContentAt(range, content).run();
}

export interface BodyExtensionOptions {
  /** What an empty body says while it can be written. */
  placeholder?: string;
  /** What an empty body says while it can only be read; nothing unless given. */
  readPlaceholder?: string;
  /**
   * The editor's own history of changes (`UndoRedo`); true unless given. A
   * body edited together has the shared document's instead, which takes
   * back the reader's own changes only (`@tiptap/extension-collaboration`).
   */
  history?: boolean;
  mention?: MentionOptions;
  emoji?: EmojiOptions;
  /** Without it references are drawn as what the reader may not see, and `[[` offers nothing. */
  reference?: Partial<ReferenceOptions>;
}

/** Every extension of the body editor. The nodes and marks are exactly those of schema.ts. */
export function bodyExtensions(options: BodyExtensionOptions = {}): AnyExtension[] {
  return [
    // Nodes
    BodyDocument,
    Paragraph,
    Text,
    Heading.configure({ levels: [1, 2, 3] }),
    BulletList,
    BodyOrderedList,
    ListItem,
    TaskList,
    BodyTaskItem.configure({ nested: true }),
    Blockquote,
    BodyCodeBlock,
    HorizontalRule,
    HardBreak,
    FileImage,
    FileBlock,
    Mention.configure(options.mention ?? {}),
    Reference.configure(options.reference ?? {}),
    InlineMath,
    BlockMath,
    BodyTable,
    BodyTableRow,
    BodyTableHeader,
    BodyTableCell,
    // Marks
    Bold,
    Italic,
    Strike,
    Code,
    BodyLink,
    // Behaviour that leaves no trace in the document
    ListKeymap,
    ...(options.history === false ? [] : [UndoRedo]),
    Dropcursor,
    Gapcursor,
    // Chosen as it is drawn: whether the editor can be written now decides.
    Placeholder.configure({
      placeholder: ({ editor }) =>
        editor.isEditable ? (options.placeholder ?? '') : (options.readPlaceholder ?? ''),
      showOnlyWhenEditable: false,
    }),
    EmojiSuggestion.configure(options.emoji ?? {}),
    EditingPlace,
    KeyReadsCaret,
  ];
}
