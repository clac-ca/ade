const localApiHost = "127.0.0.1";
const localApiPort = 8000;
const localComposeProjectName = "ade-local";
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

export {
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localComposeProjectName,
  localSqlPassword,
  localSqlPort,
  localWebPort,
};
