FROM node:22-alpine@sha256:8094c002d08262dba12645a3b4a15cd6cd627d30bc782f53229a2ec13ee22a00

WORKDIR /app
ENV NODE_ENV=production

COPY .package/ ./

EXPOSE 8001

CMD ["node", "dist/server.js"]
