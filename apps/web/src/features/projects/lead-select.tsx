import { formatPersonName, t } from "@fvoci/i18n";
import { Label } from "@/components/ui/label";
import type { components } from "@/generated/api";

type Member = components["schemas"]["MemberResponse"];

export function LeadSelect({
  id,
  value,
  members,
  onChange,
  disabled,
}: {
  id: string;
  value: string | undefined;
  members: readonly Member[];
  onChange: (userId: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="project-form__field">
      <Label htmlFor={id}>{t("project.lead")}</Label>
      <select
        id={id}
        value={value ?? ""}
        disabled={disabled || members.length === 0}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">{t("project.lead.none")}</option>
        {members.map((member) => (
          <option key={member.userId} value={member.userId}>
            {formatPersonName(member)}
          </option>
        ))}
      </select>
    </div>
  );
}
