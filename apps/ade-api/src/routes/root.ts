import process from "node:process";
import { FastifyPluginCallback, type FastifySchema } from "fastify";
import type { BundledBuildInfo } from "../config";
import { isApplicationReady, type ReadinessSnapshot } from "../readiness";

export type RootRouteOptions = {
  buildInfo: BundledBuildInfo;
  getReadinessSnapshot: () => ReadinessSnapshot;
};

const serviceStatusResponse = {
  additionalProperties: false,
  properties: {
    service: {
      const: "ade",
      type: "string",
    },
    status: {
      type: "string",
    },
  },
  required: ["service", "status"],
  type: "object",
} as const;

const rootSchema = {
  response: {
    200: {
      ...serviceStatusResponse,
      properties: {
        ...serviceStatusResponse.properties,
        version: {
          type: "string",
        },
      },
      required: [...serviceStatusResponse.required, "version"],
    },
  },
} as const satisfies FastifySchema;

const healthSchema = {
  response: {
    200: serviceStatusResponse,
  },
} as const satisfies FastifySchema;

const readinessSchema = {
  response: {
    200: {
      ...serviceStatusResponse,
      properties: {
        ...serviceStatusResponse.properties,
        status: {
          const: "ready",
          type: "string",
        },
      },
    },
    503: {
      ...serviceStatusResponse,
      properties: {
        ...serviceStatusResponse.properties,
        status: {
          const: "not-ready",
          type: "string",
        },
      },
    },
  },
} as const satisfies FastifySchema;

const versionSchema = {
  response: {
    200: {
      additionalProperties: false,
      properties: {
        builtAt: {
          type: "string",
        },
        gitSha: {
          type: "string",
        },
        nodeVersion: {
          type: "string",
        },
        service: {
          const: "ade",
          type: "string",
        },
        version: {
          type: "string",
        },
      },
      required: ["builtAt", "gitSha", "nodeVersion", "service", "version"],
      type: "object",
    },
  },
} as const satisfies FastifySchema;

const root: FastifyPluginCallback<RootRouteOptions> = (
  fastify,
  options,
  done,
): void => {
  fastify.get(
    "/",
    {
      schema: rootSchema,
    },
    () => {
      return {
        service: "ade",
        status: "ok",
        version: options.buildInfo.version,
      };
    },
  );

  fastify.get(
    "/healthz",
    {
      schema: healthSchema,
    },
    () => {
      return {
        service: "ade",
        status: "ok",
      };
    },
  );

  fastify.get(
    "/readyz",
    {
      schema: readinessSchema,
    },
    (_, reply) => {
      const readiness = options.getReadinessSnapshot();

      if (!isApplicationReady(readiness)) {
        reply.status(503);
        return {
          service: "ade",
          status: "not-ready",
        };
      }

      return {
        service: "ade",
        status: "ready",
      };
    },
  );

  fastify.get(
    "/version",
    {
      schema: versionSchema,
    },
    () => {
      return {
        ...options.buildInfo,
        nodeVersion: process.version,
      };
    },
  );

  done();
};

export default root;
