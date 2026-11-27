/** The words of the dialogs of a container's people that name its kind. */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { ContainerKind } from '$lib/logic/wire.ts';

export function shareWords(kind: ContainerKind | undefined) {
  if (kind === 'course') {
    return {
      title: m.share_course_title,
      workspaceOpen: m.share_course_workspace_open,
      guestHint: m.share_course_guest_hint,
      guestLinks: m.share_course_guest_links_title,
      guestAddresses: m.mail_course_guest_addresses_hint,
      teamRemove: m.groups_course_remove,
    };
  }
  const wiki = kind === 'wiki';
  return {
    title: wiki ? m.share_wiki_title : m['ShareBoard.Title'],
    workspaceOpen: wiki ? m.share_wiki_workspace_open : m.share_workspace_open,
    guestHint: wiki ? m.share_wiki_guest_hint : m.share_guest_hint,
    guestLinks: wiki ? m.share_wiki_guest_links_title : m.share_guest_links_title,
    guestAddresses: wiki ? m.mail_wiki_guest_addresses_hint : m.mail_guest_addresses_hint,
    teamRemove: wiki ? m.groups_wiki_remove : m.groups_board_remove,
  };
}
