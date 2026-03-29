const localApiHost = "127.0.0.1";
const localApiPort = 8000;
const localComposeProjectName = "ade-local";
const localContainerAppUrl = "http://host.docker.internal:5173";
const localSessionPoolPort = 8014;
const localSessionPoolSecret = "ade-local-session-secret";
const localSqlPassword = "AdeLocal1!adeclean";
const localSqlPort = 8013;
const localWebHost = "0.0.0.0";
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

function createLocalSessionPoolManagementEndpoint(): string {
  return `http://127.0.0.1:${String(localSessionPoolPort)}`;
}

export {
  createLocalContainerSqlConnectionString,
  localContainerAppUrl,
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localComposeProjectName,
  localSessionPoolPort,
  localSessionPoolSecret,
  localSqlPassword,
  localSqlPort,
  localWebHost,
  localWebPort,
};
