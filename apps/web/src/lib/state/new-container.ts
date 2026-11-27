/** What the window of new things asks once, for any kind, and making one from a template. */
import type { Container, ContainerPatch, ContainerVisibility } from '../logic/wire.ts';

import type { Workspace } from './workspace.svelte.ts';

/** The kinds a new thing can be made as, each from one press. */
export type NewKind = 'board' | 'wiki' | 'course' | 'diary';

/** A title (empty: the kind's own untitled name) and, in a shared space, who finds it. */
export interface ContainerDraft {
  title: string;
  visibility: ContainerVisibility;
}

/**
 * Copies a template into the workspace shown and gives the copy the draft's
 * title (when one was typed) and visibility. The copy stands even if that
 * change is refused (told).
 */
export async function createFromTemplate(
  workspace: Workspace,
  templateId: string,
  draft: ContainerDraft,
): Promise<Container | undefined> {
  const made = await workspace.duplicateContainer(templateId);
  if (!made) return undefined;
  const patch: ContainerPatch = {};
  if (draft.title && draft.title !== made.title) patch.title = draft.title;
  if (draft.visibility !== made.visibility) patch.visibility = draft.visibility;
  if (patch.title === undefined && patch.visibility === undefined) return made;
  try {
    const updated = await workspace.backend.patchContainer(made.id, patch);
    workspace.receiveContainer(updated);
    return updated;
  } catch (error) {
    workspace.report('save', error);
    return made;
  }
}
