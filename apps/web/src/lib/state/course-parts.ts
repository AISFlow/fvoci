/** Making a course in the workspace shown, in the sidebar at once. */
import type { Container } from '../logic/wire.ts';

import type { CourseDraft } from './course-types.ts';
import type { Workspace } from './workspace.svelte.ts';

export async function createCourse(
  workspace: Workspace,
  draft: CourseDraft,
): Promise<Container | undefined> {
  const courses = workspace.backend.course;
  const shown = workspace.info;
  if (!courses || !shown) return undefined;
  try {
    const made = await courses.createCourse(shown.id, draft);
    workspace.receiveContainer(made);
    return made;
  } catch (error) {
    workspace.report('save', error);
    return undefined;
  }
}
