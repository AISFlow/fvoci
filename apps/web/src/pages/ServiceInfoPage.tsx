import { useQuery } from "@tanstack/react-query";
import { loadErrorMessage, QueryError, QueryLoading } from "@/components/query-status";
import { AuthLayout } from "@/features/auth/auth-layout";
import { OperatorInfoView } from "@/features/legal/operator-info";
import { publicInstanceQuery } from "@/lib/queries/admin";

export function ServiceInfoPage() {
  const instance = useQuery(publicInstanceQuery);

  if (instance.data === undefined) {
    return (
      <AuthLayout width="wide" showWordmark={false}>
        {instance.isError ? (
          <QueryError message={loadErrorMessage(instance.error)} onRetry={() => void instance.refetch()} />
        ) : (
          <QueryLoading />
        )}
      </AuthLayout>
    );
  }

  return <OperatorInfoView operator={instance.data.values.operator} />;
}
