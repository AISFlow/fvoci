import type { EntityResolver, MentionLoader } from "@fvoci/editor/vue";
import { onScopeDispose, watch } from "vue";
import {
  createWorkspaceEditorEntities,
  type EditorEntityTransport,
} from "@/features/workspace/editor-entities";

/** Metadata follows the existing room lifetime; these callbacks exist at mount. */
export function useEditorEntities(
  workspaceId: () => string,
  roomIdentity: () => string,
  transport?: EditorEntityTransport,
) {
  let current = createWorkspaceEditorEntities(workspaceId(), transport);
  let stopped = false;
  watch(
    [workspaceId, roomIdentity],
    () => {
      current.dispose();
      current = createWorkspaceEditorEntities(workspaceId(), transport);
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    stopped = true;
    current.dispose();
  });
  const mentionItems: MentionLoader = (query) => (stopped ? [] : current.mentionItems(query));
  const entityResolver: EntityResolver = (entity, id) =>
    stopped ? Promise.resolve(null) : current.entityResolver(entity, id);
  return { mentionItems, entityResolver };
}
