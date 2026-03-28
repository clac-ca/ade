FROM python:3.12-slim

WORKDIR /app

RUN apt-get update \
    && apt-get install --yes --no-install-recommends bash \
    && rm -rf /var/lib/apt/lists/*

COPY server.py ./server.py

EXPOSE 9000

CMD ["python", "/app/server.py", "--host", "0.0.0.0", "--port", "9000", "--workspace", "/workspace"]
