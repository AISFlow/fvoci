import "@/features/collections/collections.css";

/** Source `LabelChip`: a colored dot plus the tag name. */
export function TagChip({ name, color }: { name: string; color: string }) {
  return (
    <span className="tag-chip" data-color={color}>
      {name}
    </span>
  );
}
