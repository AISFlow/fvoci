/**
 * What the share dialog works out: the roles in their order, and the direct role
 * a membership carries. The public link's address is the router's.
 */
import type { ContainerMember } from '$lib/logic/wire.ts';
import type { ContainerRole } from '$lib/state/index.ts';

/** The roles, highest first. */
export const ROLE_ORDER: readonly ContainerRole[] = ['admin', 'editor', 'commenter', 'viewer'];

/** The direct role a membership carries, apart from `inheritedRole`. */
export function roleOfMember(member: ContainerMember): ContainerRole {
  return member.role;
}
