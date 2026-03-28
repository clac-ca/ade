function createContainerRunArgs(options: {
  containerName: string;
  envNames?: readonly string[];
  hostPort: number;
  image: string;
}): string[] {
  return [
    "run",
    "--rm",
    "--name",
    options.containerName,
    "--add-host",
    "host.docker.internal:host-gateway",
    "--publish",
    `${String(options.hostPort)}:8000`,
    "--env",
    "AZURE_SQL_CONNECTIONSTRING",
    ...(options.envNames ?? []).flatMap((name) => ["--env", name]),
    options.image,
  ];
}

function createMigrationRunArgs(options: { image: string }): string[] {
  return [
    "run",
    "--rm",
    "--add-host",
    "host.docker.internal:host-gateway",
    "--env",
    "AZURE_SQL_CONNECTIONSTRING",
    options.image,
    "./bin/ade-migrate",
  ];
}

export { createContainerRunArgs, createMigrationRunArgs };
