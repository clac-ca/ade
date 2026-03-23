FROM node:22-alpine

WORKDIR /app

ENV NODE_ENV=production
ENV PORT=8000
ENV HOST=0.0.0.0

COPY apps/ade-api/.package/ ./

EXPOSE 8000

CMD ["node", "dist/server.js"]
