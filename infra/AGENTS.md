# Infra Notes

## Rule Of Thumb

- `infra/bootstrap.bicep` owns the one-time identities, OIDC, Key Vault, and Key Vault RBAC bootstrap.
- `infra/main.bicep` owns the steady-state Azure resources ADE runs inside the target resource group.
- `infra/README.md` owns the first manual deployment, the one-time SQL bootstrap, the Microsoft Entra bootstrap steps, and the GitHub `production` environment setup.
- Stable production values belong directly in the production Bicep templates.
- Tenant IDs and subscription IDs never go in tracked files.

## In Practice

- Bicep owns the deployment bootstrap, app bootstrap, VNet, subnet, Azure SQL Database, Blob Storage, Container Apps environment, Container App, migration job, ingress, and scale settings.
- The running Container App always uses the app user-assigned identity for Key Vault, SQL, Blob Storage, and session-pool access.
- The manual migration job uses the deployment managed identity.
- Use native `az bicep` and `az deployment` commands directly for infra validation and specialist deployment work.
- `infra/README.md` shows the exact direct `az`, `gh`, and Microsoft Graph PowerShell commands to bootstrap a new production resource group, seed the Key Vault secret, and hand off to the deployment pipeline.
- Runtime SQL passwords and Storage account keys are intentionally not part of the production design.
