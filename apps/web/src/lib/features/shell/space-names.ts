/**
 * How a workspace is named wherever the reader sees it named (the top bar, the
 * switcher, the share dialog, the window of new things, the hub of today):
 * the own space as "My space"; one that is its organization's only workspace
 * of the reader's by the organization's name alone; one of several by
 * "organization · workspace", and grouped under the organization in the
 * switcher.
 */
import { nameOfWorkspace } from '$lib/components/workspace-name.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import {
  isOwnSpace,
  organizationNames,
  type Workspace,
  type WorkspaceInfo,
} from '$lib/state/index.ts';

/** The name of an organization by its identifier; empty where it is not known. */
export type OrgName = (orgId: string | undefined) => string;

/** A workspace of the switcher: its line's label, and the organization it is grouped under. */
export interface SpaceLine {
  workspace: WorkspaceInfo;
  label: string;
  /** The organization's name, on the first line of its group only, for one of several workspaces. */
  heading?: string;
  /** The whole name, "organization · workspace", for one under a heading. */
  fullName: string;
  /** A line between it and the one before: it heads its organization's group, or follows one. */
  separated: boolean;
}

const titleOf = (workspace: WorkspaceInfo | undefined): string =>
  nameOfWorkspace(workspace, m.org_untitled_workspace);

/** How many of `workspaces` each organization has. */
function countsOf(workspaces: readonly WorkspaceInfo[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const one of workspaces) {
    if (one.orgId && !isOwnSpace(one)) counts.set(one.orgId, (counts.get(one.orgId) ?? 0) + 1);
  }
  return counts;
}

/**
 * The name `workspace` goes by among `workspaces`, as the top bar and the switcher's button
 * show it. One that has no title of its own and goes by it is said as `unnamed` says it.
 */
export function spaceName(
  workspace: WorkspaceInfo | undefined,
  workspaces: readonly WorkspaceInfo[],
  orgName: OrgName,
  unnamed: () => string = m.org_untitled_workspace,
): string {
  if (!workspace || isOwnSpace(workspace)) return nameOfWorkspace(workspace, unnamed);
  const org = orgName(workspace.orgId);
  if (!org || !workspace.orgId) return nameOfWorkspace(workspace, unnamed);
  if ((countsOf(workspaces).get(workspace.orgId) ?? 0) < 2) return org;
  return m.space_of_org({ org, space: titleOf(workspace) });
}

/**
 * The name the reader's workspace `info` (the one shown, where none is given) goes by
 * among theirs, with the names of their organizations as `workspace` reads them.
 */
export function shownSpaceName(
  workspace: Workspace,
  unnamed: () => string,
  info: WorkspaceInfo | undefined = workspace.info,
): string {
  const names = organizationNames(workspace);
  return spaceName(info, workspace.workspaces, (orgId) => names.of(orgId), unnamed);
}

/**
 * The switcher's lines: the own space first, then each organization's
 * workspaces together in the order the first of them is listed; an
 * organization of several is headed by its name.
 */
export function spaceLines(workspaces: readonly WorkspaceInfo[], orgName: OrgName): SpaceLine[] {
  const counts = countsOf(workspaces);
  const own = workspaces.filter(isOwnSpace);
  const shared = workspaces.filter((one) => !isOwnSpace(one));
  const orgs = [...new Set(shared.map((one) => one.orgId))];
  const lines: SpaceLine[] = own.map((workspace) => ({
    workspace,
    label: titleOf(workspace),
    fullName: titleOf(workspace),
    separated: false,
  }));
  /** The group before was an organization's of several: what follows it is set apart from it. */
  let afterGroup = false;
  for (const orgId of orgs) {
    const inOrg = shared.filter((one) => one.orgId === orgId);
    const org = orgName(orgId);
    const several = org !== '' && (counts.get(orgId ?? '') ?? 0) >= 2;
    for (const [index, workspace] of inOrg.entries()) {
      const fullName = spaceName(workspace, workspaces, orgName);
      lines.push({
        workspace,
        label: several ? titleOf(workspace) : fullName,
        fullName,
        separated: index === 0 && (several || afterGroup),
        ...(several && index === 0 ? { heading: org } : {}),
      });
    }
    afterGroup = several;
  }
  return lines;
}
