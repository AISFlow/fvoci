type EditorNode = {
  type: string;
  attrs?: Record<string, unknown>;
  content?: EditorNode[];
};

export type AttachmentNodeShape = {
  attachmentId: string;
  name: string;
  image: boolean;
};

export function attachmentNodesFromDocument(document: EditorNode): AttachmentNodeShape[] {
  const nodes: AttachmentNodeShape[] = [];
  const visit = (node: EditorNode): void => {
    if (node.type === "attachment") {
      nodes.push({
        attachmentId: typeof node.attrs?.id === "string" ? node.attrs.id : "",
        name: typeof node.attrs?.name === "string" ? node.attrs.name : "",
        image: node.attrs?.image === true,
      });
    }
    for (const child of node.content ?? []) visit(child);
  };
  visit(document);
  return nodes;
}
