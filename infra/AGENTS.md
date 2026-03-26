# Infra Notes

## Rule Of Thumb

- `infra/main.bicep` owns the Azure resources ADE runs inside the target resource group.
- `infra/README.md` owns the first manual deployment, the manual RBAC grants, the Microsoft Entra bootstrap steps, and the GitHub `production` environment setup.
- Stable production values belong in `infra/environments/main.prod.parameters.json`.
- Tenant IDs and subscription IDs never go in tracked files.

## In Practice

- Bicep owns the deployment identity, federated credential, VNet, subnet service endpoints, Azure SQL Database, Blob Storage, Container Apps environment, Container App, migration job, ingress, and scale settings.
- The running Container App uses a system-assigned managed identity.
- The manual migration job uses the deployment managed identity.
- `infra/README.md` shows the exact direct `az`, `gh`, and Microsoft Graph PowerShell commands to bootstrap a new production resource group and hand off to the deployment pipeline.
- Runtime SQL passwords and Storage account keys are intentionally not part of the production design.
