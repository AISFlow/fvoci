/**
 * What the slash menu offers (slash-menu.ts), in its order: the kinds of
 * block a line becomes or is wrapped in (three headings, a to-do list, a
 * bulleted and a numbered list, a quote, a code block, a formula on a line
 * of its own, a divider), then a table, then a file or an image where the
 * body takes files. In a table, what changes the table comes first.
 *
 * Each is offered only where it can be done (`editor.can()`): no heading
 * as the first line of a list entry, nothing a cell or a quote may not hold.
 * What is done keeps the document within what schema.ts allows.
 */
import type { Editor } from '@tiptap/core';

import { m } from '$lib/i18n/paraglide/messages.js';

import { canOpenBlockFormula, openBlockFormula } from './math-nodes.ts';
import type { SlashEntry } from './slash-menu.ts';
import { tableEntries } from './table-menu.ts';

type Level = 1 | 2 | 3;

/** An entry, with whether it can be done where the reader is now. */
interface BlockEntry extends SlashEntry {
  can: (editor: Editor) => boolean;
}

const heading = (level: Level): BlockEntry => ({
  value: `heading-${String(level)}`,
  label: m.editor_slash_heading({ level }),
  words: ['heading', 'title', `h${String(level)}`, '제목', '머리말'],
  can: (editor) => editor.can().setHeading({ level }),
  run: (editor) => editor.chain().focus().setHeading({ level }).run(),
});

/** The kinds of block, in the order the menu offers them. */
const BLOCKS: readonly (() => BlockEntry)[] = [
  () => heading(1),
  () => heading(2),
  () => heading(3),
  () => ({
    value: 'task-list',
    label: m.editor_slash_task_list(),
    words: ['todo', 'to-do', 'task', 'check', 'checkbox', '할 일 목록', '체크', '체크박스'],
    can: (editor) => editor.can().toggleTaskList(),
    run: (editor) => editor.chain().focus().toggleTaskList().run(),
  }),
  () => ({
    value: 'bullet-list',
    label: m.editor_slash_bullet_list(),
    words: ['bullet', 'list', 'unordered', '목록', '글머리'],
    can: (editor) => editor.can().toggleBulletList(),
    run: (editor) => editor.chain().focus().toggleBulletList().run(),
  }),
  () => ({
    value: 'numbered-list',
    label: m.editor_slash_numbered_list(),
    words: ['numbered', 'ordered', 'list', '번호', '목록'],
    can: (editor) => editor.can().toggleOrderedList(),
    run: (editor) => editor.chain().focus().toggleOrderedList().run(),
  }),
  () => ({
    value: 'quote',
    label: m.editor_slash_quote(),
    words: ['quote', 'blockquote', 'citation', '인용', '인용문'],
    can: (editor) => editor.can().toggleBlockquote(),
    run: (editor) => editor.chain().focus().toggleBlockquote().run(),
  }),
  () => ({
    value: 'code-block',
    label: m.editor_slash_code(),
    words: ['code', 'snippet', 'pre', '코드'],
    can: (editor) => editor.can().setCodeBlock(),
    run: (editor) => editor.chain().focus().setCodeBlock().run(),
  }),
  () => ({
    value: 'formula',
    label: m.editor_slash_formula(),
    words: ['math', 'formula', 'equation', 'latex', 'tex', '$$', '수식', '수학', '방정식'],
    can: (editor) => canOpenBlockFormula(editor.state),
    run: (editor) => openBlockFormula(editor),
  }),
  () => ({
    value: 'divider',
    label: m.editor_slash_divider(),
    words: ['divider', 'rule', 'line', 'separator', 'hr', '구분선', '가로줄', '줄'],
    can: (editor) => editor.can().setHorizontalRule(),
    run: (editor) => editor.chain().focus().setHorizontalRule().run(),
  }),
];

/** The kinds of block that can be made where the reader is now. */
export function blockEntries(editor: Editor): SlashEntry[] {
  return BLOCKS.map((made) => made()).flatMap(({ can, ...entry }) => (can(editor) ? [entry] : []));
}

/** The entry that asks for files to put in (uploads.ts), where the body takes them. */
function fileEntry(): SlashEntry {
  return {
    value: 'file',
    label: m.editor_slash_file(),
    words: [
      'file',
      'attachment',
      'upload',
      'image',
      'picture',
      'photo',
      '파일',
      '첨부',
      '이미지',
      '사진',
      '그림',
    ],
    run: (chosen) => chosen.storage.bodyUpload.choose(),
  };
}

/** Everything the slash menu offers where the reader is now. */
export function slashEntries(editor: Editor, uploads: boolean): SlashEntry[] {
  const tables = tableEntries(editor);
  const files = uploads ? [fileEntry()] : [];
  // Outside a table its one entry makes one; inside, its entries change it, and come first.
  const inTable = tables.some((entry) => entry.value !== 'table');
  return inTable
    ? [...tables, ...blockEntries(editor), ...files]
    : [...blockEntries(editor), ...tables, ...files];
}
