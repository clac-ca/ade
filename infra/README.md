# ADE Infrastructure

ADE uses one Azure template: [`infra/main.bicep`](/Users/justinkropp/.codex/worktrees/4552/ade/infra/main.bicep).

That template deploys the production resource group shape ADE runs on:

- one deployment user-assigned managed identity
- one GitHub OIDC federated credential on that deployment identity
- one VNet with a delegated Container Apps subnet that enables same-region service endpoints for Azure SQL and Blob Storage
- one Azure SQL logical server and database
- one Blob Storage account and blob container
- one Log Analytics workspace
- one VNet-integrated Azure Container Apps environment
- one public Container App for the API/web host
- one manual Azure Container Apps Job for schema migrations

The running Container App uses a **system-assigned managed identity**.
The manual migration job reuses the **deployment managed identity**.

The resource group and first deployment are manual by design because the deployment identity and its OIDC trust must exist before GitHub can deploy, and the SQL logical server identity also needs a tenant-level Microsoft Entra bootstrap step.

`infra/main.bicep` defines the migration job resource, but it does **not** run migrations by itself.

That split is intentional:

- Bicep owns declarative resource creation
- the release workflow owns the imperative "run migrations now" step

## Production names

These are the production names used by [`infra/environments/main.prod.bicepparam`](/Users/justinkropp/.codex/worktrees/4552/ade/infra/environments/main.prod.bicepparam):

- Resource group: `rg-ade-prod-canadacentral-002`
- Deployment managed identity: `id-ade-deploy-prod-canadacentral-002`
- Log Analytics workspace: `log-ade-prod-canadacentral-002`
- Container Apps environment: `cae-ade-prod-canadacentral-002`
- Container App: `ca-ade-prod-canadacentral-002`
- Migration job: `job-ade-migrate-prod-cc-002`
- Virtual network: `vnet-ade-prod-canadacentral-002`
- Azure SQL logical server: `sql-ade-prod-cc-002`
- Azure SQL database: `sqldb-ade-prod-cc-002`
- Storage account: `stadeprodcc002`
- Blob container: `documents`
- Region: `canadacentral`

## Production security model

Lock these assumptions in:

- the running app authenticates to Azure SQL with its **system-assigned managed identity**
- the running app authenticates to Blob Storage with its **system-assigned managed identity**
- the migration job authenticates to Azure SQL with the **deployment managed identity**
- the deployment managed identity is also the Azure SQL logical server's Microsoft Entra admin
- the SQL logical server itself has a **system-assigned managed identity**
- the Container Apps subnet uses **service endpoints** for `Microsoft.Sql` and same-region `Microsoft.Storage`
- Azure SQL public network access stays enabled, but access is restricted to the Container Apps subnet with a **virtual network rule**
- Blob Storage public network access stays enabled, but access is restricted to the Container Apps subnet with **storage firewall virtual network rules**
- the Azure SQL database uses the **General Purpose serverless** compute tier with auto-pause enabled
- runtime SQL passwords and Storage account keys are intentionally not part of the production design

`Microsoft.Storage.Global` is intentionally not used today. Revisit it only if ADE later needs cross-region storage access from the Container Apps subnet.

## Prerequisites

- Azure CLI installed and authenticated
- GitHub CLI installed and authenticated with admin access to `clac-ca/ade`
- PowerShell 7+ for the Microsoft Graph bootstrap step
- Permission to create resource groups, managed identities, federated credentials, role assignments, Container Apps resources, network resources, Azure SQL resources, and Storage resources in the target subscription
- Microsoft Entra `Privileged Role Administrator` or higher for the one-time SQL server identity bootstrap step

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
az provider register --namespace Microsoft.ManagedIdentity
az provider register --namespace Microsoft.Network
az provider register --namespace Microsoft.OperationalInsights
az provider register --namespace Microsoft.Sql
az provider register --namespace Microsoft.Storage
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

Set it in the environment for the Bicep parameter file:

```sh
export ADE_IMAGE=<image-ref>
```

### 6. Run the first manual deployment

```sh
az deployment group create \
  --name ade-prod-initial \
  --resource-group rg-ade-prod-canadacentral-002 \
  --parameters infra/environments/main.prod.bicepparam
```

`infra/environments/main.prod.bicepparam` reads `ADE_IMAGE` from the environment. Set it before running the deployment command.

### 7. Grant the deployment identity the minimum Azure RBAC it needs

The deployment identity needs three Azure RBAC grants:

- `Contributor` on the production resource group
- `Role Based Access Control Administrator` on the production resource group
- `Managed Identity Operator` on its own user-assigned identity resource so it can attach that identity to the migration job resource during future deployments

The current template creates one Azure RBAC resource for the running app:

- `Storage Blob Data Contributor` on the `documents` blob container for the Container App system-assigned identity

That Bicep resource is created with `Microsoft.Authorization/roleAssignments`, so the deployment identity must have permission for `Microsoft.Authorization/roleAssignments/write`.

`Role Based Access Control Administrator` is the intended grant for that. `Owner` is not required and should not be used.

Resolve the reusable values first:

```sh
export ADE_RG_ID="$(
  az group show \
    --name rg-ade-prod-canadacentral-002 \
    --query id \
    --output tsv
)"

export ADE_DEPLOYMENT_IDENTITY_ID="$(
  az identity show \
    --name id-ade-deploy-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query id \
    --output tsv
)"

export ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID="$(
  az identity show \
    --name id-ade-deploy-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query principalId \
    --output tsv
)"
```

Grant `Contributor` on the resource group:

```sh
az role assignment create \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --assignee-principal-type ServicePrincipal \
  --role Contributor \
  --scope "${ADE_RG_ID}"
```

Grant `Role Based Access Control Administrator` on the resource group:

```sh
az role assignment create \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --assignee-principal-type ServicePrincipal \
  --role "Role Based Access Control Administrator" \
  --scope "${ADE_RG_ID}"
```

Grant `Managed Identity Operator` on the deployment identity resource itself:

```sh
az role assignment create \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --assignee-principal-type ServicePrincipal \
  --role "Managed Identity Operator" \
  --scope "${ADE_DEPLOYMENT_IDENTITY_ID}"
```

Allow several minutes for Azure RBAC propagation before validating the bootstrap or re-running the deployment pipeline.

### 8. Validate the bootstrap RBAC and manual deployment

List the deployment identity role assignments on the production resource group:

```sh
az role assignment list \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --scope "${ADE_RG_ID}" \
  --output table
```

Confirm the output includes:

- `Contributor`
- `Role Based Access Control Administrator`

List the deployment identity role assignments on the deployment identity resource itself:

```sh
az role assignment list \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --scope "${ADE_DEPLOYMENT_IDENTITY_ID}" \
  --output table
```

Confirm the output includes:

- `Managed Identity Operator`

Re-run the manual deployment after the RBAC grants have propagated:

```sh
export ADE_IMAGE=<image-ref>

az deployment group create \
  --name ade-prod-rbac-validate \
  --resource-group rg-ade-prod-canadacentral-002 \
  --parameters infra/environments/main.prod.bicepparam
```

Resolve the running app principal ID and the blob container ID:

```sh
export ADE_APP_PRINCIPAL_ID="$(
  az containerapp show \
    --name ca-ade-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query identity.principalId \
    --output tsv
)"

export ADE_BLOB_CONTAINER_ID="$(
  az storage container show \
    --name documents \
    --account-name stadeprodcc002 \
    --auth-mode login \
    --query id \
    --output tsv
)"
```

Confirm the Bicep-created storage RBAC assignment exists:

```sh
az role assignment list \
  --assignee-object-id "${ADE_APP_PRINCIPAL_ID}" \
  --scope "${ADE_BLOB_CONTAINER_ID}" \
  --output table
```

Confirm the output includes:

- `Storage Blob Data Contributor`

After the manual validation succeeds, re-run the GitHub deployment pipeline and confirm `Release Stage` proceeds past `Deploy accepted release candidate`.

### 9. Grant the SQL logical server identity Microsoft Entra query permissions

The Azure SQL logical server uses a **system-assigned managed identity**.

That identity must be able to resolve Microsoft Entra principals for `CREATE USER ... FROM EXTERNAL PROVIDER` to work.

Grant it the **Directory Readers** role, or the documented lower-level Microsoft Graph permissions:

- `User.Read.All`
- `GroupMember.Read.All`
- `Application.Read.All`

This is a **tenant-level** bootstrap step. It is not handled by Bicep.

The most direct first-time path is the Microsoft Graph PowerShell flow from the Azure SQL documentation. Run it as a Microsoft Entra `Privileged Role Administrator` or higher:

```powershell
$TenantId = "<tenant-id>"
$ServerName = "sql-ade-prod-cc-002"

Connect-MgGraph -TenantId $TenantId -Scopes "RoleManagement.ReadWrite.Directory","Application.Read.All"

$roleName = "Directory Readers"
$role = Get-MgDirectoryRole -Filter "DisplayName eq '$roleName'"
if ($null -eq $role) {
    $roleTemplate = Get-MgDirectoryRoleTemplate -Filter "DisplayName eq '$roleName'"
    New-MgDirectoryRoleTemplate -RoleTemplateId $roleTemplate.Id
    $role = Get-MgDirectoryRole -Filter "DisplayName eq '$roleName'"
}

$roleMember = Get-MgServicePrincipal -Filter "DisplayName eq '$ServerName'"
if ($null -eq $roleMember) {
    throw "No service principal found for SQL server '$ServerName'."
}

if ($roleMember.Count -ne 1) {
    throw "Multiple service principals found for SQL server '$ServerName'."
}

$isMember = Get-MgDirectoryRoleMember -DirectoryRoleId $role.Id -Filter "Id eq '$($roleMember.Id)'"
if ($null -eq $isMember) {
    New-MgDirectoryRoleMemberByRef `
      -DirectoryRoleId $role.Id `
      -BodyParameter @{
        '@odata.id' = "https://graph.microsoft.com/v1.0/directoryObjects/$($roleMember.Id)"
      }
}
```

### 10. Set the GitHub `production` environment variables

Set the deployment identity client ID:

```sh
gh variable set AZURE_CLIENT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "$(
    az identity show \
      --name id-ade-deploy-prod-canadacentral-002 \
      --resource-group rg-ade-prod-canadacentral-002 \
      --query clientId \
      --output tsv
  )"
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

### 11. Run the initial migration job manually

The first manual deployment defines the migration job, but it does not execute it.

Run it once manually after the SQL server identity has Directory Readers:

```sh
az containerapp job start \
  --name job-ade-migrate-prod-cc-002 \
  --resource-group rg-ade-prod-canadacentral-002
```

Inspect recent executions:

```sh
az containerapp job execution list \
  --name job-ade-migrate-prod-cc-002 \
  --resource-group rg-ade-prod-canadacentral-002 \
  --output table
```

At steady state, that same start-and-poll sequence is handled by `release_stage` in the deployment pipeline.

## Re-running the deployment

After the bootstrap is complete, future pushes to `main` should use the deployment pipeline.

If you need to reconcile production manually, re-run the template directly with a published image:

```sh
export ADE_IMAGE=<image-ref>

az deployment group create \
  --name ade-prod-reconcile \
  --resource-group rg-ade-prod-canadacentral-002 \
  --parameters infra/environments/main.prod.bicepparam
```

If that deployment changes the migration job or the application image in a way that requires schema changes, start the migration job explicitly afterward.

## Operational notes

- The Container Apps environment is VNet-integrated. Azure does not let you switch an environment from the default network mode to VNet integration in place, so treat the current environment shape as intentional.
- The release workflow deploys the accepted image first and then starts the migration job. Keep schema changes backward-compatible and use expand/contract migrations by default.
- The app readiness endpoint must remain process-level. Do not turn `/api/readyz` into a "latest schema is present" gate.
