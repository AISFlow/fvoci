const ORDER = ["guest", "member", "admin", "owner"] as const;

/** Workspace role rank used by the React settings page. */
export function roleAtLeast(role: string, minimum: string): boolean {
  return ORDER.indexOf(role as (typeof ORDER)[number]) >= ORDER.indexOf(minimum as (typeof ORDER)[number]);
}
