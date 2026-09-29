import type { DataTag, DefaultError, QueryKey } from "@tanstack/query-core";

/**
 * Options for a query both web apps run while React and Vue coexist, limited
 * to the fields whose types the two adapters agree on: vue-query reads a
 * function `enabled` as a reactive getter and re-types every callback that
 * receives the query key, so neither appears here. Add fields such as
 * `select` or `placeholderData` at the call site, with the adapter's types.
 */
export interface SharedQueryOptions<TQueryFnData, TQueryKey extends QueryKey> {
  queryKey: TQueryKey;
  queryFn: (context: { signal: AbortSignal }) => Promise<TQueryFnData>;
  enabled?: boolean;
  retry?: boolean | number;
  staleTime?: number;
  refetchInterval?: number | false;
}

/**
 * Same as @tanstack/react-query's and @tanstack/vue-query's `queryOptions`:
 * returns `options` unchanged, with `queryKey` tagged with the data the query
 * caches so `getQueryData`, `setQueryData` and friends infer it. Typed against
 * the shared @tanstack/query-core, so both adapters accept the result.
 */
export function queryOptions<TQueryFnData, const TQueryKey extends QueryKey>(
  options: SharedQueryOptions<TQueryFnData, TQueryKey>,
): SharedQueryOptions<TQueryFnData, TQueryKey> & { queryKey: DataTag<TQueryKey, TQueryFnData, DefaultError> } {
  return options as SharedQueryOptions<TQueryFnData, TQueryKey> & {
    queryKey: DataTag<TQueryKey, TQueryFnData, DefaultError>;
  };
}
