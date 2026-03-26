import process from "node:process";
import { readMigrationConfig } from "./config";
import { runMigrations } from "./db/migrate-runner";
import { createConsoleLogger, runMain } from "./process";

type MainDependencies = {
  env?: NodeJS.ProcessEnv;
  run?: typeof runMigrations;
};

async function main(
  logger = createConsoleLogger(),
  dependencies: MainDependencies = {},
) {
  const config = readMigrationConfig(dependencies.env ?? process.env);
  const run = dependencies.run ?? runMigrations;

  const result = await run({
    connectionString: config.sqlConnectionString,
  });

  for (const migrationName of result.applied) {
    logger.info(`Applied migration: ${migrationName}`);
  }

  for (const migrationName of result.skipped) {
    logger.info(`Skipped migration: ${migrationName}`);
  }

  logger.info(
    `Migration complete. Applied ${String(result.applied.length)}, skipped ${String(result.skipped.length)}.`,
  );
}

if (require.main === module) {
  void runMain(async () => {
    await main();
  });
}

export { main };
