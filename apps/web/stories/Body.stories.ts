import type { Meta, StoryObj } from '@storybook/svelte';

import BodySample from './BodySample.svelte';
import {
  DIAGRAMS,
  FILES,
  FORMULAS,
  FROM_TEMPLATE,
  INVALID,
  MIXED,
  REFERENCES,
  TOGGLES,
} from './body-samples.ts';

/** The read-only view of a document body (src/lib/editor/BodyView.svelte). */
const meta = {
  title: 'Editor/Body',
  component: BodySample,
} satisfies Meta<typeof BodySample>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Every kind of node and mark a body may contain; an image opens larger on a press, or Enter. */
export const Mixed: Story = { args: { doc: MIXED } };
/** One document of the built-in templates. */
export const FromTemplate: Story = { args: { doc: FROM_TEMPLATE } };
/** A body the validator refuses: nothing of it is drawn, the caller's notice is. */
export const Invalid: Story = { args: { doc: INVALID } };
/** No body at all. */
export const Empty: Story = { args: { doc: null } };
/** Formulas, drawn once the formula library has been fetched; a body without any never fetches it. */
export const Formulas: Story = { args: { doc: FORMULAS } };
/** Diagrams, drawn with the colours of the page and again when the theme changes. */
export const Diagrams: Story = { args: { doc: DIAGRAMS } };
/** References to cards and pages: with a title, too long for the line, not the reader's to see, and being asked. */
export const References: Story = { args: { doc: REFERENCES } };
/** Toggles, closed at first (the page's own `<details>`), and a callout of each tone. */
export const Toggles: Story = { args: { doc: TOGGLES } };
/** File blocks: the kind, name and size of each file and a button that saves it; a file that is gone says so. */
export const Files: Story = { args: { doc: FILES } };
