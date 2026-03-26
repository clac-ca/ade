export class ApiError extends Error {
  readonly statusCode: number;

  constructor(message: string, statusCode: number) {
    super(message);
    this.name = "ApiError";
    this.statusCode = statusCode;
  }
}

function normalizeApiPath(path: string) {
  const trimmed = path.trim();

  if (trimmed === "") {
    throw new Error("API paths must not be empty.");
  }

  if (trimmed === "/api" || trimmed.startsWith("/api/")) {
    return trimmed;
  }

  return `/api${trimmed.startsWith("/") ? trimmed : `/${trimmed}`}`;
}

export async function apiFetch<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const headers = new Headers(init?.headers);

  if (!headers.has("accept")) {
    headers.set("accept", "application/json");
  }

  const response = await fetch(normalizeApiPath(path), {
    ...init,
    headers,
  });

  if (!response.ok) {
    let message = `Request failed with status ${String(response.status)}.`;

    try {
      const payload = (await response.json()) as {
        message?: unknown;
      };

      if (
        typeof payload.message === "string" &&
        payload.message.trim() !== ""
      ) {
        message = payload.message;
      }
    } catch {
      // Keep the fallback message if the error body is not JSON.
    }

    throw new ApiError(message, response.status);
  }

  return (await response.json()) as T;
}
