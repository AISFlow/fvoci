import type { components } from "@/generated/api";

export type SetupInput = components["schemas"]["SetupBody"];
export type LoginInput = components["schemas"]["LoginBody"];
export type WorkspaceCreateInput = components["schemas"]["CreateWorkspaceBody"];
export type WorkspaceRenameInput = Pick<
  components["schemas"]["PatchWorkspaceBody"],
  "name"
> & {
  name: string;
};
