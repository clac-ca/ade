# Infra Notes

## Rule Of Thumb

- `infra/main.bicep` owns the Azure resources ADE runs inside the target resource group.
- `infra/README.md` owns the first manual deployment, the manual RBAC grants, and the GitHub `production` environment setup.
- Stable production values belong in `infra/environments/main.prod.bicepparam`.
- Secrets, tenant IDs, and subscription IDs never go in tracked files.

## In Practice

- Bicep owns the deployment identity, runtime identity, federated credential, Key Vault, Container Apps environment, Log Analytics workspace, Container App, ingress, scale settings, and identity attachment.
- `infra/README.md` shows the exact direct `az` and `gh` commands to bootstrap a new production resource group and hand off to the deployment pipeline.
- Secret values belong in Key Vault, not in Bicep, workflow YAML, or parameter files.
