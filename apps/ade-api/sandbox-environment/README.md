## Sandbox Environment

This directory is the source of truth for the shared ADE sandbox environment.
It is an `ade-api` component, not a standalone package.

- `rootfs/` is the authored filesystem tree packaged into `sandbox-environment.tar.gz` during `pnpm build`.
- `python-version.txt` pins the Python runtime staged directly into `/app/ade/python/current` inside that archive.
- `build.ts` is the co-located build implementation that turns this component into one tarball carried by the API image.
- `packages/reverse-connect/Dockerfile.build` is the build-only Linux artifact recipe used to inject the `reverse-connect` binary into that tarball at build time.

The shared sandbox environment is separate from config installation.

- `setup.sh` prepares the shared runtime only and never downloads Python from the internet.
- The API installs the selected config directly from its mounted sandbox path under `/mnt/data/ade/configs/<workspaceId>/<configVersionId>/`.
- The API executes `ade process` directly after install.
- The sandbox/session container itself stays vanilla. The API uploads the tarball, extracts it, and executes the copied `reverse-connect` binary there.

This directory contains only app-owned runtime assets.

- `reverse-connect` source code stays in `packages/reverse-connect` because it is reusable code with its own tests and binary output.
- Its Linux binary is exported from `packages/reverse-connect/Dockerfile.build` and then copied into the sandbox-environment tarball during `pnpm build:sandbox-environment`.
- Config wheels do not belong here because they are installed separately after prepare.
- Generated runtime additions such as `reverse-connect`, the pinned Python runtime, and the base wheelhouse are injected at build time and written only to the tarball output.
