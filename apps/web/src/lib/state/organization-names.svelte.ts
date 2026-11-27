/**
 * The names of the reader's organizations, which the list of their workspaces
 * does not carry: what tells the workspaces of one organization from those of
 * another (the switcher, the top bar, the notifications). One for each
 * `Workspace`, read again when the organizations of its workspaces change.
 */
import type { UserOrganization, WorkspaceInfo } from './backend.ts';
import type { Workspace } from './workspace.svelte.ts';

export class OrganizationNames {
  #organizations = $state.raw<readonly UserOrganization[]>([]);
  /** The organizations last read for, as one key: a list with the same reads nothing. */
  #readFor = '';
  readonly #read: () => Promise<UserOrganization[]>;

  constructor(read: () => Promise<UserOrganization[]>) {
    this.#read = read;
  }

  /** The organization's name; empty while it is not known, or where it was given none. */
  of(orgId: string | undefined): string {
    return orgId === undefined
      ? ''
      : (this.#organizations.find((one) => one.id === orgId)?.name ?? '');
  }

  /** Takes a name the organization was given here, at once: the list is not read again for it. */
  renamed(orgId: string, name: string): void {
    this.#organizations = this.#organizations.map((one) =>
      one.id === orgId ? { ...one, name } : one,
    );
  }

  /** Reads the names for the organizations of `workspaces`, unless they were read for those. */
  follow(workspaces: readonly WorkspaceInfo[]): void {
    const key = workspaces
      .flatMap((one) => (one.orgId ? [one.orgId] : []))
      .filter((orgId, index, all) => all.indexOf(orgId) === index)
      .sort()
      .join(' ');
    if (key === this.#readFor) return;
    this.#readFor = key;
    if (key === '') return;
    this.#read().then(
      (organizations) => {
        if (this.#readFor !== key) return;
        this.#organizations = organizations;
      },
      () => {
        // Unread names leave each workspace its own name: nothing is lost but the grouping.
        if (this.#readFor === key) this.#readFor = '';
      },
    );
  }
}

const ofWorkspace = new WeakMap<Workspace, OrganizationNames>();

/** The names of the organizations of the reader that `workspace` lists. */
export function organizationNames(workspace: Workspace): OrganizationNames {
  let names = ofWorkspace.get(workspace);
  if (!names) {
    names = new OrganizationNames(() => workspace.backend.organizations());
    ofWorkspace.set(workspace, names);
  }
  return names;
}
