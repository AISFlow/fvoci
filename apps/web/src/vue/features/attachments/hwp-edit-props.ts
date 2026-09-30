/**
 * Session edit context (source `hwpEditable` + `onSavedCopy`). `editable`
 * comes from `GET …/edit-context`; `save` is present only in a workspace
 * session, and the server re-checks edit access when the copy is written.
 * A share view passes none of it.
 */
export type HwpEditProps = {
  editable: boolean;
  save?: {
    workspaceId: string;
    attachmentId: string;
    onSavedCopy: (attachmentId: string) => void | Promise<void>;
  };
};
