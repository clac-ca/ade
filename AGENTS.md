# AGENTS

```text
.
├── compose.yaml
├── README.md
├── PRINCIPLES.md
├── infra/
│   ├── main.bicep
│   ├── environments/
│   └── modules/
├── scripts/
│   ├── acceptance.mjs
│   ├── build.mjs
│   ├── dev.mjs
│   ├── deploy-aca.mjs
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
pnpm lint
pnpm test
pnpm test:unit
pnpm test:acceptance
pnpm clean
pnpm dev:web
pnpm dev:api
```
