import { createApiClient, installApiClient } from "@/lib/api";

/** Node unit tests need an absolute base URL for openapi-fetch Request construction. */
export function installNodeApiClient(baseUrl = "http://fvoci.test"): void {
  installApiClient(createApiClient({ baseUrl }));
}
