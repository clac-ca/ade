# AGENTS

```text
.
├── Dockerfile
├── README.md
├── PRINCIPLES.md
├── tsconfig.base.json
├── tsconfig.scripts.json
├── infra/
│   ├── local/
│   │   └── compose.yaml
│   ├── main.bicep
│   ├── environments/
│   └── modules/
├── scripts/
│   ├── acceptance.ts
│   ├── build.ts
│   ├── clean.ts
│   ├── dev.ts
│   ├── local-deps.ts
│   ├── start.ts
│   ├── lib/
│   └── test/
├── apps/
│   ├── ade-api/
│   └── ade-web/
└── packages/
    ├── ade-engine/
    └── ade-config/
```

```sh
pnpm install
pnpm deps:up
pnpm deps:down
pnpm dev
pnpm build
pnpm start
pnpm lint
pnpm format:check
pnpm test
pnpm test:unit
pnpm test:acceptance
pnpm package:python
pnpm clean
```
