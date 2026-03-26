type AppErrorOptions = {
  cause?: unknown;
  exitCode?: number;
  expose?: boolean;
  statusCode?: number;
};

class AppError extends Error {
  readonly exitCode: number;
  readonly expose: boolean;
  readonly statusCode?: number;

  constructor(message: string, options: AppErrorOptions = {}) {
    super(message, {
      cause: options.cause,
    });
    this.name = new.target.name;
    this.exitCode = options.exitCode ?? 1;
    this.expose = options.expose ?? true;

    if (options.statusCode !== undefined) {
      this.statusCode = options.statusCode;
    }
  }
}

class ConfigError extends AppError {
  constructor(message: string, options: AppErrorOptions = {}) {
    super(message, {
      exitCode: options.exitCode ?? 2,
      expose: options.expose ?? true,
      ...(options.statusCode !== undefined
        ? {
            statusCode: options.statusCode,
          }
        : {}),
      ...(options.cause !== undefined
        ? {
            cause: options.cause,
          }
        : {}),
    });
  }
}

type RouteErrorOptions = {
  cause?: unknown;
  expose?: boolean;
  statusCode?: number;
};

class DatabaseError extends AppError {
  constructor(message: string, cause?: unknown) {
    super(message, {
      cause,
      exitCode: 3,
      expose: true,
      statusCode: 503,
    });
  }
}

class StartupError extends AppError {
  constructor(message: string, cause?: unknown) {
    super(message, {
      cause,
      exitCode: 4,
      expose: true,
      statusCode: 503,
    });
  }
}

class RouteError extends AppError {
  constructor(message: string, options: RouteErrorOptions = {}) {
    const statusCode = options.statusCode ?? 500;

    super(message, {
      cause: options.cause,
      expose: options.expose ?? statusCode < 500,
      statusCode,
    });
  }
}

function toRouteError(error: unknown): RouteError {
  if (error instanceof RouteError) {
    return error;
  }

  if (error instanceof AppError) {
    return new RouteError(error.message, {
      cause: error.cause,
      expose: error.expose,
      statusCode: error.statusCode ?? 500,
    });
  }

  return new RouteError("Internal Server Error", {
    cause: error,
    expose: false,
    statusCode: 500,
  });
}

export { AppError, ConfigError, DatabaseError, RouteError, StartupError, toRouteError };
