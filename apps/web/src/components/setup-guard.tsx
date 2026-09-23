import { useQuery } from "@tanstack/react-query";
import { Navigate } from "react-router-dom";
import { setupStatusQuery } from "@/lib/queries";

export function SetupGuard({ children }: { children: React.ReactNode }) {
  const setup = useQuery(setupStatusQuery);
  if (setup.isLoading) {
    return <p role="status">...</p>;
  }
  if (setup.isError || setup.data?.needed) {
    return <Navigate to="/setup" replace />;
  }
  return children;
}
