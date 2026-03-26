import { apiFetch } from "./client";

export type ApiVersion = {
  service: string;
  version: string;
};

export function getVersion() {
  return apiFetch<ApiVersion>("/version");
}
