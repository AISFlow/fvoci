import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { starsQuery } from "@/lib/queries/share";
import { toggleStar } from "./share-api";

/** Source cmdk star/unstar action, surfaced as a toggle on the item page. */
export function StarToggle({
  workspaceId,
  type,
  targetId,
}: {
  workspaceId: string;
  type: "document" | "task";
  targetId: string;
}) {
  const queryClient = useQueryClient();
  const stars = useQuery(starsQuery(workspaceId));
  const [error, setError] = useState<string | null>(null);
  const star = stars.data?.items.find((item) => item.type === type && item.targetId === targetId);

  const toggle = useMutation({
    mutationFn: () => toggleStar(workspaceId, star, type, targetId),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey: ["stars", workspaceId] });
    },
    onError: (err: unknown) => {
      setError(problemMessage(err, "load.failed"));
    },
  });

  return (
    <>
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={stars.isLoading || toggle.isPending}
        onClick={() => toggle.mutate()}
      >
        <span aria-hidden className="mr-1">
          {star ? "★" : "☆"}
        </span>
        {star ? t("cmdk.unstar") : t("cmdk.star")}
      </Button>
      {error ? (
        <p role="alert" className="document-page__error">
          {error}
        </p>
      ) : null}
    </>
  );
}
