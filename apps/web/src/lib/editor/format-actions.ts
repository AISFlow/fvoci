/**
 * What the two bars of the editor do (SelectionBubbleBar.svelte over chosen
 * text, MobileEditorToolbar.svelte over a phone's keyboard): each a command
 * with its name, its icon, whether it is on where the selection is, and
 * whether it can be done there now. The bars draw these and know nothing of
 * the document.
 */
import BadgeInfoIcon from '@lucide/svelte/icons/badge-info';
import BoldIcon from '@lucide/svelte/icons/bold';
import CodeIcon from '@lucide/svelte/icons/code';
import Heading1Icon from '@lucide/svelte/icons/heading-1';
import Heading2Icon from '@lucide/svelte/icons/heading-2';
import Heading3Icon from '@lucide/svelte/icons/heading-3';
import ItalicIcon from '@lucide/svelte/icons/italic';
import ListIcon from '@lucide/svelte/icons/list';
import ListCollapseIcon from '@lucide/svelte/icons/list-collapse';
import ListOrderedIcon from '@lucide/svelte/icons/list-ordered';
import ListTodoIcon from '@lucide/svelte/icons/list-todo';
import PaperclipIcon from '@lucide/svelte/icons/paperclip';
import QuoteIcon from '@lucide/svelte/icons/quote';
import Redo2Icon from '@lucide/svelte/icons/redo-2';
import SigmaIcon from '@lucide/svelte/icons/sigma';
import StrikethroughIcon from '@lucide/svelte/icons/strikethrough';
import TableIcon from '@lucide/svelte/icons/table';
import Undo2Icon from '@lucide/svelte/icons/undo-2';
import type { Editor } from '@tiptap/core';
import type { Component } from 'svelte';

import { m } from '$lib/i18n/paraglide/messages.js';

import { inlineFormulaOf } from './math-nodes.ts';
import type { SlashEntry } from './slash-menu.ts';
import { tableEntries } from './table-menu.ts';

export interface FormatAction {
  /** Tells it apart in the page (`data-format`). */
  id: string;
  label: () => string;
  /** Drawn beside the name, or alone with the name said aloud: never read itself. */
  icon: Component<{ class?: string; 'aria-hidden'?: boolean }>;
  /** Whether what it does is on where the selection is now; a toggle shows it pressed. */
  active?: (editor: Editor) => boolean;
  /** Whether it can be done now. */
  can: (editor: Editor) => boolean;
  run: (editor: Editor) => void;
}

/** A mark of the text chosen, which a press puts on or takes off. */
function mark(
  id: string,
  name: string,
  label: () => string,
  icon: FormatAction['icon'],
): FormatAction {
  return {
    id,
    label,
    icon,
    active: (editor) => editor.isActive(name),
    can: (editor) => editor.can().toggleMark(name),
    run: (editor) => {
      editor.chain().focus().toggleMark(name).run();
    },
  };
}

/** The marks over chosen text, before the link (which the bubble asks the address of). */
export const MARK_ACTIONS: readonly FormatAction[] = [
  mark('bold', 'bold', m.editor_format_bold, BoldIcon),
  mark('italic', 'italic', m.editor_format_italic, ItalicIcon),
  mark('strike', 'strike', m.editor_format_strike, StrikethroughIcon),
  mark('code', 'code', m.editor_format_code, CodeIcon),
];

/** The chosen text made a formula in its line, its text the source. */
export const FORMULA_ACTION: FormatAction = {
  id: 'formula',
  label: m.editor_format_formula,
  icon: SigmaIcon,
  can: (editor) => inlineFormulaOf(editor.state) !== null,
  run: (editor) => {
    const made = inlineFormulaOf(editor.state);
    if (made) editor.view.dispatch(made);
    editor.view.focus();
  },
};

const HEADINGS = { 1: Heading1Icon, 2: Heading2Icon, 3: Heading3Icon } as const;

const heading = (level: 1 | 2 | 3): FormatAction => ({
  id: `heading-${String(level)}`,
  label: () => m.editor_slash_heading({ level }),
  icon: HEADINGS[level],
  active: (editor) => editor.isActive('heading', { level }),
  can: (editor) => editor.can().toggleHeading({ level }),
  run: (editor) => {
    editor.chain().focus().toggleHeading({ level }).run();
  },
});

/** A kind of block the line is turned into, or out of again. */
function block(
  id: string,
  name: string,
  label: () => string,
  icon: FormatAction['icon'],
  toggle: (editor: Editor) => boolean,
  can: (editor: Editor) => boolean,
): FormatAction {
  return { id, label, icon, active: (editor) => editor.isActive(name), can, run: toggle };
}

/** The slash menu's entry that makes a table, where one can be made. */
const newTable = (editor: Editor): SlashEntry | undefined =>
  tableEntries(editor).find((entry) => entry.value === 'table');

/** What the bar over a phone's keyboard does to the line, in its order. */
export function lineActions(uploads: boolean): FormatAction[] {
  return [
    heading(1),
    heading(2),
    heading(3),
    block(
      'task-list',
      'taskList',
      m.editor_slash_task_list,
      ListTodoIcon,
      (editor) => editor.chain().focus().toggleTaskList().run(),
      (editor) => editor.can().toggleTaskList(),
    ),
    block(
      'bullet-list',
      'bulletList',
      m.editor_slash_bullet_list,
      ListIcon,
      (editor) => editor.chain().focus().toggleBulletList().run(),
      (editor) => editor.can().toggleBulletList(),
    ),
    block(
      'numbered-list',
      'orderedList',
      m.editor_slash_numbered_list,
      ListOrderedIcon,
      (editor) => editor.chain().focus().toggleOrderedList().run(),
      (editor) => editor.can().toggleOrderedList(),
    ),
    block(
      'quote',
      'blockquote',
      m.editor_slash_quote,
      QuoteIcon,
      (editor) => editor.chain().focus().toggleBlockquote().run(),
      (editor) => editor.can().toggleBlockquote(),
    ),
    block(
      'toggle',
      'details',
      m.editor_slash_toggle,
      ListCollapseIcon,
      (editor) => editor.chain().focus().toggleDetails().run(),
      (editor) => editor.can().toggleDetails(),
    ),
    block(
      'callout',
      'callout',
      m.editor_slash_callout,
      BadgeInfoIcon,
      (editor) => editor.chain().focus().toggleCallout().run(),
      (editor) => editor.can().toggleCallout(),
    ),
    {
      id: 'table',
      label: m.editor_slash_table,
      icon: TableIcon,
      // Where the slash menu makes one: on a line of the document itself (table-menu.ts).
      can: (editor) => newTable(editor) !== undefined,
      run: (editor) => {
        newTable(editor)?.run(editor);
      },
    },
    ...(uploads
      ? [
          {
            id: 'file',
            label: m.editor_slash_file,
            icon: PaperclipIcon,
            can: (editor: Editor) => editor.isEditable,
            run: (editor: Editor) => {
              editor.storage.bodyUpload.choose();
            },
          },
        ]
      : []),
  ];
}

/** Undo and redo: the editor's own history, or the shared document's (only the reader's changes). */
export const HISTORY_ACTIONS: readonly FormatAction[] = [
  {
    id: 'undo',
    label: m['UndoRedoHotKeys.canUndo'],
    icon: Undo2Icon,
    can: (editor) => editor.can().undo(),
    run: (editor) => {
      editor.chain().focus().undo().run();
    },
  },
  {
    id: 'redo',
    label: m['UndoRedoHotKeys.canRedo'],
    icon: Redo2Icon,
    can: (editor) => editor.can().redo(),
    run: (editor) => {
      editor.chain().focus().redo().run();
    },
  },
];
