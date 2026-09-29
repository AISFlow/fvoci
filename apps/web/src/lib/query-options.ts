import type {
  DefaultError,
  QueryKey,
  QueryKeyWithDataTag,
  QueryObserverOptions,
} from "@tanstack/query-core";

/**
 * Query options for both web apps while React and Vue coexist:
 * @tanstack/react-query and @tanstack/vue-query both accept the object. Same
 * as their own `queryOptions` (an identity function that only tags
 * `queryKey` with the cached data type, so `getQueryData` and friends infer
 * it), typed against the shared @tanstack/query-core instead of either
 * adapter. For options without `initialData`, which is all the shared
 * factories use.
 */
export function queryOptions<
  TQueryFnData = unknown,
  TError = DefaultError,
  TData = TQueryFnData,
  TQueryKey extends QueryKey = QueryKey,
>(
  options: QueryObserverOptions<TQueryFnData, TError, TData, TQueryFnData, TQueryKey> & {
    initialData?: undefined;
  },
): QueryObserverOptions<TQueryFnData, TError, TData, TQueryFnData, TQueryKey> &
  QueryKeyWithDataTag<TQueryKey, TQueryFnData, TError> {
  return options as QueryObserverOptions<TQueryFnData, TError, TData, TQueryFnData, TQueryKey> &
    QueryKeyWithDataTag<TQueryKey, TQueryFnData, TError>;
}
