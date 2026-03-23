# AGENTS

```text
.
├── Dockerfile
├── README.md
├── PRINCIPLES.md
├── infra/
│   ├── main.bicep
│   ├── environments/
│   └── modules/
├── scripts/
│   ├── acceptance.mjs
│   ├── build.mjs
│   ├── clean.mjs
│   ├── dev.mjs
│   ├── start.mjs
│   └── shared.mjs
├── apps/
│   ├── ade-api/
│   └── ade-web/
└── packages/
    ├── ade-engine/
    └── ade-config/
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
pnpm package:python
pnpm clean
```
