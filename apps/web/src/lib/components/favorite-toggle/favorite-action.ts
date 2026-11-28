/**
 * The ★ as a line of a menu, where a bar or a list has no room for its button:
 * "Add to favorites" or "Remove from favorites", as the reader's stars say;
 * none until they are read, nor where none are kept.
 */
import StarIcon from '@lucide/svelte/icons/star';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

import type { ReaderStars } from './reader-stars.ts';

/** The line that puts a star on `targetId`, or takes it off. */
export function favoriteAction(
  stars: ReaderStars | undefined,
  targetId: string,
): MenuAction | undefined {
  const starred = stars?.has(targetId);
  if (!stars || starred === undefined) return undefined;
  return {
    id: 'favorite',
    label: starred ? m.favorite_remove() : m.favorite_add(),
    icon: StarIcon,
    run: () => void stars.toggle(targetId),
  };
}
