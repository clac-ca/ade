FROM node:22-alpine

WORKDIR /app
ENV NODE_ENV=production

COPY .package/ ./

EXPOSE 8001

CMD ["node", "dist/server.js"]
