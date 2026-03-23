# Infra Notes

## Rule Of Thumb

- Put ADE workload infrastructure in Bicep.
- Keep one-time Azure and GitHub trust bootstrap manual and documented.
- Do not commit tenant IDs, subscription IDs, secrets, or other bootstrap-only identifiers into tracked files.

## In Practice

- Bicep owns the Container Apps environment, Log Analytics workspace, Container App, ingress, scale settings, identity attachment, and later Key Vault references.
- `bootstrap/README.md` owns the one-time setup for the resource group, deployment identity, runtime identity, Key Vault creation, role assignments, federated credential, and GitHub `production` environment variables.
- Secret values belong in Key Vault, not in Bicep, workflow YAML, or parameter files.
