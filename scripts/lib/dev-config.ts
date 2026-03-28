const localApiHost = "127.0.0.1";
const localApiPort = 8000;
const localComposeProjectName = "ade-local";
const localSessionPoolPort = 8014;
const localSessionPoolRuntimeSecret = "ade-local-session-secret";
const localSqlPassword = "AdeLocal1!adeclean";
const localSqlPort = 8013;
const localWebPort = 5173;

function createLocalSqlConnectionString(): string {
  return [
    `Server=127.0.0.1,${String(localSqlPort)}`,
    "Database=ade",
    "User Id=sa",
    `Password=${localSqlPassword}`,
    "Encrypt=false",
    "TrustServerCertificate=true",
  ].join(";");
}

function createLocalContainerSqlConnectionString(): string {
  return [
    `Server=host.docker.internal,${String(localSqlPort)}`,
    "Database=ade",
    "User Id=sa",
    `Password=${localSqlPassword}`,
    "Encrypt=false",
    "TrustServerCertificate=true",
  ].join(";");
}

function createLocalContainerSessionPoolManagementEndpoint(): string {
  return `http://host.docker.internal:${String(localSessionPoolPort)}`;
}

function createLocalContainerSessionPoolMcpEndpoint(): string {
  return `${createLocalContainerSessionPoolManagementEndpoint()}/mcp`;
}

function createLocalSessionPoolManagementEndpoint(): string {
  return `http://127.0.0.1:${String(localSessionPoolPort)}`;
}

function createLocalSessionPoolMcpEndpoint(): string {
  return `${createLocalSessionPoolManagementEndpoint()}/mcp`;
}

export {
  createLocalContainerSqlConnectionString,
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalContainerSessionPoolMcpEndpoint,
  createLocalSessionPoolManagementEndpoint,
  createLocalSessionPoolMcpEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localComposeProjectName,
  localSessionPoolPort,
  localSessionPoolRuntimeSecret,
  localSqlPassword,
  localSqlPort,
  localWebPort,
};
