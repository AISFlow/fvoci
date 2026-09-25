import type { components } from "@/generated/api";

export type SetupInput = components["schemas"]["SetupBody"];
export type LoginInput = components["schemas"]["LoginBody"];
export type WorkspaceCreateInput = components["schemas"]["CreateWorkspaceBody"];
export type WorkspaceRenameInput = components["schemas"]["PatchWorkspaceBody"];
export type WorkspaceRole = "owner" | "admin" | "member" | "guest";
export type MemberOutput = components["schemas"]["MemberResponse"];
export type InvitationCreateInput = components["schemas"]["InvitationCreateBody"];
export type InvitationPublicOutput = components["schemas"]["InvitationPublicResponse"];
export type InvitationAcceptInput = components["schemas"]["InvitationAcceptBody"];
export type ApiTokenOutput = components["schemas"]["ApiTokenOutput"];
export type ApiTokenCreatedOutput = components["schemas"]["ApiTokenCreatedOutput"];
export type ApiTokenCreateInput = components["schemas"]["ApiTokenCreateBody"];
export type ApiTokenScope = NonNullable<ApiTokenCreateInput["scopes"]>[number];
