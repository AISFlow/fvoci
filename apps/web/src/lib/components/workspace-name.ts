/**
 * The name a workspace is shown by. The reader's own space has none and
 * takes none: it is "My space" in the reader's language. Another that was
 * given none is shown by `unnamed`, which says it as its place needs.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

export function nameOfWorkspace(
  workspace: { readonly title: string; readonly kind?: string } | undefined,
  unnamed: () => string,
): string {
  if (workspace?.kind === 'personal') return m.own_space_name();
  return workspace?.title || unnamed();
}
