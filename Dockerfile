FROM node:24-alpine

WORKDIR /app

ENV NODE_ENV=production

COPY apps/ade-api/.package/ ./

EXPOSE 8000

CMD ["node", "dist/server.js", "--host", "0.0.0.0", "--port", "8000"]
