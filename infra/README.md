# ADE Infrastructure

ADE uses one Azure template: [`infra/main.bicep`](/Users/justinkropp/.codex/worktrees/4552/ade/infra/main.bicep).

That template creates the Azure resources ADE runs on inside the production resource group:

- deployment user-assigned managed identity
- runtime user-assigned managed identity
- GitHub OIDC federated credential on the deployment identity
- Key Vault
- Log Analytics workspace
- Container Apps environment
- Container App

The resource group is still created manually. The first deployment is also manual by design.

Why:

- GitHub cannot deploy the Azure resources until the deployment identity already exists
- GitHub cannot log in with OIDC until that identity is trusted and its client ID is configured in the `production` environment

After the first manual deployment and the manual trust setup, the deployment pipeline takes over.

Do not commit these values into tracked files:

- tenant ID
- subscription ID
- secret values

Keep tenant ID and subscription ID only in the GitHub `production` environment variables used by the deployment pipeline.

## Production names

These are the production names used by [`infra/environments/main.prod.bicepparam`](/Users/justinkropp/.codex/worktrees/4552/ade/infra/environments/main.prod.bicepparam):

- Resource group: `rg-ade-prod-canadacentral-002`
- Deployment managed identity: `id-ade-deploy-prod-canadacentral-002`
- Runtime managed identity: `id-ade-runtime-prod-canadacentral-002`
- Key Vault: `kv-ade-prod-cc-002`
- Log Analytics workspace: `log-ade-prod-canadacentral-002`
- Container Apps environment: `cae-ade-prod-canadacentral-002`
- Container App: `ca-ade-prod-canadacentral-002`
- Region: `canadacentral`

## Prerequisites

- Azure CLI installed and authenticated
- GitHub CLI installed and authenticated with admin access to `clac-ca/ade`
- Permission to create resource groups, managed identities, federated credentials, role assignments, Key Vaults, and Container Apps resources in the target subscription

## First-time setup

### 1. Sign in to Azure

```sh
az login
```

### 2. Select the target subscription

```sh
az account set --subscription <subscription-id>
```

Confirm the active account:

```sh
az account show --query '{tenantId:tenantId,subscriptionId:id,name:name}' --output table
```

### 3. Register the required providers

```sh
az provider register --namespace Microsoft.App
az provider register --namespace Microsoft.OperationalInsights
az provider register --namespace Microsoft.Insights
az provider register --namespace Microsoft.ManagedIdentity
az provider register --namespace Microsoft.KeyVault
```

### 4. Create the production resource group

```sh
az group create \
  --name rg-ade-prod-canadacentral-002 \
  --location canadacentral
```

### 5. Choose a published image for the first deployment

Use a published ADE image from public GHCR.

The image format is:

```text
ghcr.io/clac-ca/ade:sha-<commit-sha>
```

### 6. Run the first manual deployment

Replace `<image-ref>` with the published ADE image you want to deploy.

```sh
export ADE_IMAGE=<image-ref>
```

```sh
az deployment group create \
  --name ade-prod-initial \
  --resource-group rg-ade-prod-canadacentral-002 \
  --parameters infra/environments/main.prod.bicepparam
```

`infra/environments/main.prod.bicepparam` reads `ADE_IMAGE` from the environment. Set it before running the deployment command.

This deployment creates:

- the deployment managed identity
- the runtime managed identity
- the GitHub federated credential on the deployment identity
- the Key Vault
- the Log Analytics workspace
- the Container Apps environment
- the Container App

### 7. Grant the deployment identity the minimum deployment access

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

Grant `Managed Identity Operator` on the runtime identity so the deployment identity can attach it to the Container App:

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

Grant `Key Vault Secrets User` on the Key Vault to the runtime identity:

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

### 8. Set the GitHub `production` environment variables

Set the deployment identity client ID:

```sh
gh variable set AZURE_CLIENT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "$(az identity show \
    --name id-ade-deploy-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query clientId \
    --output tsv)"
```

Set the tenant ID:

```sh
gh variable set AZURE_TENANT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "<tenant-id>"
```

Set the subscription ID:

```sh
gh variable set AZURE_SUBSCRIPTION_ID \
  --env production \
  --repo clac-ca/ade \
  --body "<subscription-id>"
```

Set the resource group:

```sh
gh variable set AZURE_RESOURCE_GROUP \
  --env production \
  --repo clac-ca/ade \
  --body "rg-ade-prod-canadacentral-002"
```

List the environment variables to confirm:

```sh
gh variable list --env production --repo clac-ca/ade
```

## Rebuild the Container App from scratch

This is the cleanest way to prove the template can recreate the app resource.

Delete the existing Container App:

```sh
az containerapp delete \
  --name ca-ade-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --yes
```

Redeploy the template with the same published image:

```sh
export ADE_IMAGE=<image-ref>
```

```sh
az deployment group create \
  --name ade-prod-recreate-app \
  --resource-group rg-ade-prod-canadacentral-002 \
  --parameters infra/environments/main.prod.bicepparam
```

Confirm the app is back:

```sh
az containerapp show \
  --name ca-ade-prod-canadacentral-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --query '{name:name,image:properties.template.containers[0].image,ingress:properties.configuration.ingress.fqdn,minReplicas:properties.template.scale.minReplicas}' \
  --output json
```

Confirm readiness:

```sh
curl -fsS https://ca-ade-prod-canadacentral-002.lemonriver-5a158f92.canadacentral.azurecontainerapps.io/api/readyz
```

## After setup

After the first manual deployment, RBAC grants, and GitHub environment setup:

- pushes to `main` run the Deployment Pipeline
- the release stage logs into Azure with OIDC
- the release stage deploys the accepted image to `rg-ade-prod-canadacentral-002`

The GitHub `production` environment should contain exactly these variables:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`
