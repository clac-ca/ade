import createClient from "openapi-fetch";
import type { paths } from "./schema";

export class ApiError extends Error {
  readonly statusCode: number;

  constructor(message: string, statusCode: number) {
    super(message);
    this.name = "ApiError";
    this.statusCode = statusCode;
  }
}

export function createApiClient(
  baseUrl: string,
  fetchImpl: typeof globalThis.fetch = globalThis.fetch,
) {
  return createClient<paths>({
    baseUrl,
    fetch(request) {
      return fetchImpl(request);
    },
  });
}

export const apiClient = createApiClient(window.location.origin);
