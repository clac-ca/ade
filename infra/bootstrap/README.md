# ADE Azure Bootstrap

This document bootstraps the Azure side of ADE's production deployment.

It follows a simple split:

- GitHub Actions authenticates to Azure with OIDC through a user-assigned managed identity
- the running Container App gets a separate user-assigned managed identity for Key Vault access
- the app image stays on public GHCR for now

This is a one-time manual setup. Use direct `az` commands only.

Do not commit the tenant ID or subscription ID into this repo.

- Do not hardcode them in Bicep
- Do not hardcode them in workflow YAML
- Do not put them in parameter files
- Store them only in the GitHub `production` environment variables:
  - `AZURE_TENANT_ID`
  - `AZURE_SUBSCRIPTION_ID`

## Names used in this first production setup

- Resource group: `rg-ade-prod-canadacentral-002`
- Deployment managed identity: `id-ade-deploy-prod-canadacentral-002`
- Runtime managed identity: `id-ade-runtime-prod-canadacentral-002`
- Container Apps environment: `cae-ade-prod-canadacentral-002`
- Container App: `ca-ade-prod-canadacentral-002`
- Log Analytics workspace: `log-ade-prod-canadacentral-002`
- Key Vault: `kv-ade-prod-cc-002`
- Region: `canadacentral`

## Prerequisites

- Azure CLI installed and up to date
- GitHub CLI installed and authenticated with admin access to `clac-ca/ade`
- Permission to create identities, role assignments, and Key Vault resources in the target subscription

## 1. Sign in and select the subscription

Sign in interactively:

```sh
az login
```

Select the target subscription:

```sh
az account set --subscription <subscription-id>
```

Confirm the active tenant and subscription:

```sh
az account show --query '{tenantId:tenantId,subscriptionId:id,name:name}' --output table
```

## 2. Register the required resource providers

These providers are used by Container Apps, Log Analytics, managed identities, Insights, and Key Vault.

```sh
az provider register --namespace Microsoft.App
az provider register --namespace Microsoft.OperationalInsights
az provider register --namespace Microsoft.Insights
az provider register --namespace Microsoft.ManagedIdentity
az provider register --namespace Microsoft.KeyVault
```

## 3. Create the production resource group

This is the scope the deployment identity will manage.

```sh
az group create \
  --name rg-ade-prod-canadacentral-002 \
  --location canadacentral
```

## 4. Create the deployment managed identity

GitHub Actions will federate to this identity through OIDC.

```sh
az identity create \
  --name id-ade-deploy-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --location canadacentral
```

## 5. Create the runtime managed identity

The Container App will use this identity at runtime for Key Vault access.

```sh
az identity create \
  --name id-ade-runtime-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --location canadacentral
```

## 6. Create the Key Vault

Use Azure RBAC, not access policies.

```sh
az keyvault create \
  --name kv-ade-prod-cc-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --location canadacentral \
  --enable-rbac-authorization true
```

## 7. Grant the deployment identity access to deploy resources

Grant `Contributor` on the production resource group:

```sh
az role assignment create \
  --assignee-object-id "$(az identity show \
    --name id-ade-deploy-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query principalId \
    --output tsv)" \
  --assignee-principal-type ServicePrincipal \
  --role Contributor \
  --scope "$(az group show \
    --name rg-ade-prod-canadacentral-002 \
    --query id \
    --output tsv)"
```

Grant `Managed Identity Operator` on the runtime identity so deployments can attach it to the Container App:

```sh
az role assignment create \
  --assignee-object-id "$(az identity show \
    --name id-ade-deploy-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query principalId \
    --output tsv)" \
  --assignee-principal-type ServicePrincipal \
  --role "Managed Identity Operator" \
  --scope "$(az identity show \
    --name id-ade-runtime-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query id \
    --output tsv)"
```

## 8. Grant the runtime identity access to Key Vault secrets

Grant `Key Vault Secrets User` on the vault:

```sh
az role assignment create \
  --assignee-object-id "$(az identity show \
    --name id-ade-runtime-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query principalId \
    --output tsv)" \
  --assignee-principal-type ServicePrincipal \
  --role "Key Vault Secrets User" \
  --scope "$(az keyvault show \
    --name kv-ade-prod-cc-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query id \
    --output tsv)"
```

## 9. Create the GitHub federated credential on the deployment identity

This trusts only the GitHub `production` environment for `clac-ca/ade`.

```sh
az identity federated-credential create \
  --name github-production \
  --identity-name id-ade-deploy-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --issuer https://token.actions.githubusercontent.com \
  --subject repo:clac-ca/ade:environment:production \
  --audiences api://AzureADTokenExchange
```

## 10. Read back the deployment identity client ID

GitHub uses the client ID in `azure/login`.

```sh
az identity show \
  --name id-ade-deploy-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --query clientId \
  --output tsv
```

## 11. Set the GitHub `production` environment variables

Use the client ID from the previous step.

```sh
gh variable set AZURE_CLIENT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "<deployment-identity-client-id>"
```

```sh
gh variable set AZURE_TENANT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "<tenant-id>"
```

```sh
gh variable set AZURE_SUBSCRIPTION_ID \
  --env production \
  --repo clac-ca/ade \
  --body "<subscription-id>"
```

```sh
gh variable set AZURE_RESOURCE_GROUP \
  --env production \
  --repo clac-ca/ade \
  --body "rg-ade-prod-canadacentral-002"
```

List the variables to confirm:

```sh
gh variable list --env production --repo clac-ca/ade
```

## 12. Validate the bootstrap

Confirm both identities exist:

```sh
az identity show \
  --name id-ade-deploy-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --output table
```

```sh
az identity show \
  --name id-ade-runtime-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --output table
```

Confirm the federated credential exists:

```sh
az identity federated-credential list \
  --identity-name id-ade-deploy-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --output table
```

Confirm role assignments:

```sh
az role assignment list \
  --scope "$(az group show --name rg-ade-prod-canadacentral-002 --query id --output tsv)" \
  --all \
  --output table
```

```sh
az role assignment list \
  --scope "$(az identity show --name id-ade-runtime-prod-canadacentral-002 --resource-group rg-ade-prod-canadacentral-002 --query id --output tsv)" \
  --all \
  --output table
```

```sh
az role assignment list \
  --scope "$(az keyvault show --name kv-ade-prod-cc-002 --resource-group rg-ade-prod-canadacentral-002 --query id --output tsv)" \
  --all \
  --output table
```

## What happens after this

After this bootstrap:

- GitHub Actions logs into Azure with OIDC and no client secret
- the production deployment targets `rg-ade-prod-canadacentral-002`
- the deployment workflow derives the runtime identity resource ID at runtime
- the Container App can be attached to `id-ade-runtime-prod-canadacentral-002`
- Key Vault is ready for secret references in the next pass
