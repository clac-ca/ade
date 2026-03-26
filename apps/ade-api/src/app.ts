import { existsSync } from "node:fs";
import { join } from "node:path";
import fastifyStatic from "@fastify/static";
import Fastify, { FastifyInstance } from "fastify";
import { toRouteError } from "./errors";
import rootRoute, { RootRouteOptions } from "./routes/root";

const apiNotFoundSchema = {
  response: {
    404: {
      additionalProperties: false,
      properties: {
        error: {
          const: "Not Found",
          type: "string",
        },
        message: {
          type: "string",
        },
        statusCode: {
          const: 404,
          type: "number",
        },
      },
      required: ["error", "message", "statusCode"],
      type: "object",
    },
  },
} as const;

const apiErrorSchema = {
  additionalProperties: false,
  properties: {
    error: {
      type: "string",
    },
    message: {
      type: "string",
    },
    statusCode: {
      type: "number",
    },
  },
  required: ["error", "message", "statusCode"],
  type: "object",
} as const;

type CreateAppOptions = RootRouteOptions & {
  logger?: boolean;
  webRoot?: string;
};

function createApp({
  logger = true,
  webRoot,
  ...options
}: CreateAppOptions): FastifyInstance {
  const server = Fastify({
    routerOptions: {
      ignoreTrailingSlash: true,
    },
    logger,
  });

  server.setErrorHandler(async (error, _, reply) => {
    const routeError = toRouteError(error);
    const statusCode = routeError.statusCode ?? 500;

    reply.status(statusCode);
    return {
      error: statusCode >= 500 ? "Internal Server Error" : "Request Error",
      message: routeError.expose ? routeError.message : "Internal Server Error",
      statusCode,
    };
  });

  server.register(rootRoute, {
    ...options,
    prefix: "/api",
  });

  server.all(
    "/api/*",
    {
      schema: apiNotFoundSchema,
    },
    async (request, reply) => {
      const requestPath = request.url.split("?", 1)[0] ?? request.url;

      reply.status(404);
      return {
        error: "Not Found",
        message: `Route ${request.method}:${requestPath} not found`,
        statusCode: 404,
      };
    },
  );

  if (webRoot && existsSync(join(webRoot, "index.html"))) {
    server.register(fastifyStatic, {
      root: webRoot,
    });

    server.get("/", async (_, reply) => {
      return reply.sendFile("index.html");
    });

    server.setNotFoundHandler(async (request, reply) => {
      const requestPath = request.url.split("?", 1)[0] ?? request.url;

      if (
        requestPath === "/api" ||
        requestPath.startsWith("/api/") ||
        /\.[^/]+$/.test(requestPath)
      ) {
        reply.status(404);
        return {
          error: "Not Found",
          message: `Route ${request.method}:${requestPath} not found`,
          statusCode: 404,
        };
      }

      return reply.sendFile("index.html");
    });
  } else {
    server.setNotFoundHandler(async (request, reply) => {
      reply.status(404);
      return {
        error: "Not Found",
        message: `Route ${request.method}:${request.url} not found`,
        statusCode: 404,
      };
    });
  }

  return server;
}

export { apiErrorSchema, apiNotFoundSchema, createApp };
