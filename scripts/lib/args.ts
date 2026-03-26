import { parseArgs as parseNodeArgs } from "node:util";

type BrowserArgs = {
  noOpen: boolean;
  port: number;
};

type StartArgs = BrowserArgs & {
  image: string;
};

type AcceptanceArgs = {
  url: URL;
};

function parsePort(value: string, name: string): number {
  if (!/^[1-9]\d*$/.test(value)) {
    throw new Error(`Invalid ${name}: ${value}`);
  }

  const port = Number.parseInt(value, 10);

  if (port > 65_535) {
    throw new Error(`Invalid ${name}: ${value}`);
  }

  return port;
}

function parseBrowserArgs(
  argv: readonly string[],
  defaultPort: number,
): BrowserArgs {
  const { values } = parseNodeArgs({
    allowPositionals: false,
    args: argv,
    options: {
      "no-open": {
        type: "boolean",
      },
      port: {
        type: "string",
      },
    },
    strict: true,
  });

  return {
    noOpen: values["no-open"] ?? false,
    port:
      values.port === undefined ? defaultPort : parsePort(values.port, "port"),
  };
}

function parseDevArgs(argv: readonly string[]): BrowserArgs {
  return parseBrowserArgs(argv, 5173);
}

function parseStartArgs(argv: readonly string[]): StartArgs {
  const { values } = parseNodeArgs({
    allowPositionals: false,
    args: argv,
    options: {
      image: {
        type: "string",
      },
      "no-open": {
        type: "boolean",
      },
      port: {
        type: "string",
      },
    },
    strict: true,
  });

  return {
    image: values.image?.trim() ? values.image : "ade:local",
    noOpen: values["no-open"] ?? false,
    port: values.port === undefined ? 8000 : parsePort(values.port, "port"),
  };
}

function parseAcceptanceArgs(argv: readonly string[]): AcceptanceArgs {
  const { values } = parseNodeArgs({
    allowPositionals: false,
    args: argv,
    options: {
      url: {
        type: "string",
      },
    },
    strict: true,
  });
  const value = values.url?.trim();

  if (!value) {
    throw new Error("Missing required --url");
  }

  try {
    return {
      url: new URL(value),
    };
  } catch (error) {
    throw new Error(`Invalid --url: ${value}`, {
      cause: error,
    });
  }
}

export { parseAcceptanceArgs, parseDevArgs, parsePort, parseStartArgs };

export type { AcceptanceArgs, BrowserArgs, StartArgs };
