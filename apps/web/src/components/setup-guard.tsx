import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Navigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { setupStatusQuery } from "@/lib/queries";

export function SetupGuard({ children }: { children: React.ReactNode }) {
  const setup = useQuery(setupStatusQuery);
  if (setup.isLoading) {
    return <p role="status">{t("load.loading")}</p>;
  }
  if (setup.isError) {
    return (
      <div className="p-8">
        <p role="alert" className="text-muted-foreground">{t("load.failed")}</p>
        <Button type="button" size="sm" className="mt-2" onClick={() => void setup.refetch()}>
          {t("load.retry")}
        </Button>
      </div>
    );
  }
  if (setup.data?.needed) {
    return <Navigate to="/setup" replace />;
  }
  return children;
}
