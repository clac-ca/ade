import { ApiError, apiClient } from "./client";
import type { components } from "./schema";

type VersionInfo = components["schemas"]["VersionResponse"];

export type VersionClient = {
  GET(
    path: "/api/version",
    init: Record<string, never>,
  ): Promise<{
    data?: VersionInfo;
    error?: unknown;
    response: Response;
  }>;
};

function errorMessage(error: unknown, statusCode: number): string {
  if (typeof error !== "object" || error === null) {
    return `Request failed with status ${String(statusCode)}.`;
  }

  const message = (error as { message?: unknown }).message;

  if (typeof message === "string" && message.trim() !== "") {
    return message;
  }

  return `Request failed with status ${String(statusCode)}.`;
}

export async function getVersion(
  client: VersionClient = apiClient,
): Promise<VersionInfo> {
  const result = await client.GET("/api/version", {});

  if (result.error !== undefined) {
    throw new ApiError(
      errorMessage(result.error, result.response.status),
      result.response.status,
    );
  }

  if (result.data === undefined) {
    throw new ApiError(
      `Request failed with status ${String(result.response.status)}.`,
      result.response.status,
    );
  }

  return result.data;
}
