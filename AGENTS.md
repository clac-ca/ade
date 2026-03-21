# AGENTS

```text
.
├── README.md
├── PRINCIPLES.md              # durable engineering principles
├── apps/
│   ├── web/
│   └── api/
├── packages/
│   └── contracts/
└── python/
    ├── ade-engine/            # stable extraction engine package
    └── ade-config-template/   # customizable ADE config template package
```

```sh
pnpm install
pnpm check # root validation
pnpm --filter @ade/web dev
pnpm --filter @ade/api dev
```
