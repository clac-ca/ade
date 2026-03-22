# AGENTS

```text
.
├── compose.yaml
├── README.md
├── PRINCIPLES.md
├── scripts/
│   ├── dev.mjs
│   ├── smoke-start.mjs
│   ├── start.mjs
│   └── shared.mjs
├── apps/
│   ├── web/
│   │   ├── Dockerfile
│   │   └── nginx.conf
│   └── api/
│       └── Dockerfile
└── python/
    ├── ade-engine/
    └── ade-config-template/
```

```sh
pnpm install
pnpm dev
pnpm build
pnpm start
pnpm check
pnpm clean
pnpm dev:web
pnpm dev:api
```
