import type { DataTag, DefaultError, InfiniteData, QueryKey } from "@tanstack/query-core";

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
  retry?: SharedRetry;
  staleTime?: number;
  refetchInterval?: number | false;
}

/** Retry count, switch, or predicate: the same query-core type in both adapters. */
export type SharedRetry =
  boolean | number | ((failureCount: number, error: DefaultError) => boolean);

/**
 * Same as @tanstack/react-query's and @tanstack/vue-query's `queryOptions`:
 * returns `options` unchanged, with `queryKey` tagged with the data the query
 * caches so `getQueryData`, `setQueryData` and friends infer it. Typed against
 * the shared @tanstack/query-core, so both adapters accept the result.
 */
export function queryOptions<TQueryFnData, const TQueryKey extends QueryKey>(
  options: SharedQueryOptions<TQueryFnData, TQueryKey>,
): SharedQueryOptions<TQueryFnData, TQueryKey> & {
  queryKey: DataTag<TQueryKey, TQueryFnData, DefaultError>;
} {
  return options as SharedQueryOptions<TQueryFnData, TQueryKey> & {
    queryKey: DataTag<TQueryKey, TQueryFnData, DefaultError>;
  };
}

/** An infinite (paged) query both web apps run; same limits as {@link SharedQueryOptions}. */
export interface SharedInfiniteQueryOptions<TQueryFnData, TQueryKey extends QueryKey, TPageParam> {
  queryKey: TQueryKey;
  queryFn: (context: { signal: AbortSignal; pageParam: TPageParam }) => Promise<TQueryFnData>;
  initialPageParam: TPageParam;
  getNextPageParam: (lastPage: TQueryFnData) => TPageParam | undefined | null;
  enabled?: boolean;
  retry?: SharedRetry;
  staleTime?: number;
}

/**
 * Same as the adapters' `infiniteQueryOptions`: returns `options` unchanged,
 * with `queryKey` tagged with the paged data the query caches.
 */
export function infiniteQueryOptions<TQueryFnData, const TQueryKey extends QueryKey, TPageParam>(
  options: SharedInfiniteQueryOptions<TQueryFnData, TQueryKey, TPageParam>,
): SharedInfiniteQueryOptions<TQueryFnData, TQueryKey, TPageParam> & {
  queryKey: DataTag<TQueryKey, InfiniteData<TQueryFnData, TPageParam>, DefaultError>;
} {
  return options as SharedInfiniteQueryOptions<TQueryFnData, TQueryKey, TPageParam> & {
    queryKey: DataTag<TQueryKey, InfiniteData<TQueryFnData, TPageParam>, DefaultError>;
  };
}
