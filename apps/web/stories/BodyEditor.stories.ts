import type { Meta, StoryObj } from '@storybook/svelte';

import BodyEditorSample from './BodyEditorSample.svelte';
import { DIAGRAMS, FILES, FORMULAS, INVALID, MIXED, REFERENCES, TOGGLES } from './body-samples.ts';

/** The editor of a document body (src/lib/editor/BodyEditor.svelte), with the view of what it would store. */
const meta = {
  title: 'Editor/BodyEditor',
  component: BodyEditorSample,
} satisfies Meta<typeof BodyEditorSample>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * An empty body: type `/` for the kinds of block (`/제목`, `/ㅎㅇ`, `/quote`), `@` for
 * people, `:` for emoji, `$x^2$` for a formula, ```` ```mermaid ```` for a diagram.
 * Choose some text for the bubble of marks; on a phone, a bar over the keyboard.
 */
export const Empty: Story = { args: { doc: null } };
/** Every kind of node and mark a body had before formulas. */
export const Mixed: Story = { args: { doc: MIXED } };
/**
 * Formulas: press one to change its source. One in a line opens a small box;
 * one on a line of its own opens its source under it, as a diagram does.
 */
export const Formulas: Story = { args: { doc: FORMULAS } };
/**
 * Diagrams: the picture alone. Point at one (or select it) and it offers to be
 * edited; press it, or Enter while it is selected, and its source opens under
 * it, redrawn as it is typed. Escape, Ctrl+Enter or a press elsewhere closes.
 * A source that cannot be drawn shows, with why, where the picture would be.
 */
export const Diagrams: Story = { args: { doc: DIAGRAMS } };
/** Diagrams where the body is only read: the pictures, which do not open. */
export const DiagramsReadOnly: Story = { args: { doc: DIAGRAMS, editable: false } };
/** Not editable. */
export const ReadOnly: Story = { args: { doc: FORMULAS, editable: false } };
/** A body the validator refuses is not loaded: the editor stays empty and read-only, and says so. */
export const Unreadable: Story = { args: { doc: INVALID } };
/** References: type `[[` for the cards and pages there are to name; each shows what it names as it is now. */
export const References: Story = { args: { doc: REFERENCES } };
/**
 * File blocks: type `/` for the slash menu and choose "파일/이미지", or paste or drop a
 * file that is no picture. Select a block with the arrows and Enter saves its file;
 * select an image and its bubble opens it larger.
 */
export const Files: Story = { args: { doc: FILES } };
/**
 * Toggles and callouts: the chevron opens a toggle for this reader alone, Enter in a
 * summary goes on in what it hides, and a callout's icon changes its tone.
 */
export const Toggles: Story = { args: { doc: TOGGLES } };
