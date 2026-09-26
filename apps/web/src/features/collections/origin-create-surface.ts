export type OriginCreateSurface =
  | "loading"
  | "error"
  | "create-task"
  | "create-project"
  | "unavailable"
  | "pending";

/** Empty picker: members can create a project; guests with document View cannot. */
export function originCreateSurface(input: {
  isLoading: boolean;
  isError: boolean;
  itemCount?: number;
  canCreateProject?: boolean;
}): OriginCreateSurface {
  if (input.isLoading) return "loading";
  if (input.isError) return "error";
  if (input.itemCount === undefined) return "pending";
  if (input.itemCount > 0) return "create-task";
  return input.canCreateProject ? "create-project" : "unavailable";
}
