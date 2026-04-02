## Sandbox Environment

This directory is the source of truth for the shared ADE sandbox environment.
It is an `ade-api` component, not a standalone package.

- `rootfs/` is the authored filesystem tree packaged into `sandbox-environment.tar.gz` during `pnpm build`.
- `python-version.txt` pins the Python runtime staged directly into `/mnt/data/ade/python/current` inside that archive.
- `build.ts` is the co-located build implementation that turns this component into one tarball carried by the API image.

The shared sandbox environment is separate from config installation.

- `setup.sh` prepares the shared runtime only and never downloads Python from the internet.
- The API installs the selected config directly from its mounted sandbox path.
- The API executes `ade process` directly after install.

This directory contains only app-owned runtime assets.

- `reverse-connect` source code stays in `packages/reverse-connect` because it is reusable code with its own tests and binary output.
- Config wheels do not belong here because they are installed separately after prepare.
- Generated runtime additions such as `reverse-connect`, the pinned Python runtime, and the base wheelhouse are injected at build time and written only to the tarball output.
