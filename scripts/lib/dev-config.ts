const localApiHost = "127.0.0.1";
const localApiPort = 8000;
const localBlobAccountKey =
  "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
const localBlobAccountName = "devstoreaccount1";
const localBlobContainerName = "documents";
const localBlobPort = 10000;
const localComposeProjectName = "ade-local";
const localSessionPoolPort = 8014;
const localSessionPoolBearerToken = "ade-local-session-token";
const localSessionPoolSecret = "ade-local-session-secret";
const localSqlPassword = "AdeLocal1!adeclean";
const localSqlPort = 8013;
const localWebHost = "0.0.0.0";
const localWebPort = 5173;
const localContainerAppUrl = createLocalContainerAppUrl(localApiPort);

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

function createLocalBlobAccountUrl(): string {
  return `http://127.0.0.1:${String(localBlobPort)}/${localBlobAccountName}`;
}

function createLocalContainerAppUrl(port: number): string {
  return `http://host.docker.internal:${String(port)}`;
}

function createLocalContainerBlobAccountUrl(): string {
  return `http://host.docker.internal:${String(localBlobPort)}/${localBlobAccountName}`;
}

function createLocalContainerSessionPoolManagementEndpoint(): string {
  return `http://host.docker.internal:${String(localSessionPoolPort)}`;
}

function createLocalSessionPoolManagementEndpoint(): string {
  return `http://127.0.0.1:${String(localSessionPoolPort)}`;
}

export {
  createLocalContainerSqlConnectionString,
  createLocalBlobAccountUrl,
  createLocalContainerAppUrl,
  createLocalContainerBlobAccountUrl,
  localContainerAppUrl,
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localBlobAccountKey,
  localBlobAccountName,
  localBlobContainerName,
  localBlobPort,
  localComposeProjectName,
  localSessionPoolBearerToken,
  localSessionPoolPort,
  localSessionPoolSecret,
  localSqlPassword,
  localSqlPort,
  localWebHost,
  localWebPort,
};
