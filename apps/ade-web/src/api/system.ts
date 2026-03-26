import { apiFetch } from "./client";

export type ApiVersion = {
  builtAt: string;
  gitSha: string;
  runtimeVersion: string;
  service: string;
  version: string;
};

export function getVersion() {
  return apiFetch<ApiVersion>("/version");
}
