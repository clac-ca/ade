# ADE Infrastructure

ADE uses one Azure template: [`main.bicep`](main.bicep).
Local laptop and CI dependency services live separately in [`local/compose.yaml`](local/compose.yaml); that file is not part of the Azure deployment model.

The template deploys:

- one deployment user-assigned managed identity
- one GitHub OIDC federated credential on that deployment identity
- one VNet with a delegated Container Apps subnet
- one Azure SQL logical server and database
- one Blob Storage account and blob container placeholder
- one Log Analytics workspace
- one VNet-integrated Azure Container Apps environment
- one public Container App for the API/web host
- one manual Azure Container Apps Job for schema migrations

The running Container App uses a system-assigned managed identity.
The manual migration job reuses the deployment managed identity.

`infra/main.bicep` defines the migration job resource, but it does not run migrations by itself.
The public Container App uses the image's default startup command and never runs schema migrations on startup.
The migration job is the only Azure resource that overrides its command, and it does so with `./bin/ade-migrate`.

## Production names

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

- the running app authenticates to Azure SQL with its system-assigned managed identity
- the migration job authenticates to Azure SQL with the deployment managed identity
- the deployment managed identity is also the Azure SQL logical server's Microsoft Entra admin
- the SQL logical server itself has a system-assigned managed identity
- the Container Apps subnet uses service endpoints for `Microsoft.Sql` and same-region `Microsoft.Storage`
- Azure SQL public network access stays enabled, but access is restricted to the Container Apps subnet with a virtual network rule
- Blob Storage stays provisioned as a placeholder, but it is not an active application dependency today
- the Azure SQL database uses the General Purpose serverless compute tier with auto-pause enabled

## Prerequisites

- Azure CLI installed and authenticated
- GitHub CLI installed and authenticated with admin access to `clac-ca/ade`
- PowerShell 7+ for the Microsoft Graph bootstrap step
- Permission to create resource groups, managed identities, federated credentials, Container Apps resources, network resources, Azure SQL resources, and Storage resources in the target subscription
- Microsoft Entra `Privileged Role Administrator` or higher for the one-time SQL server identity bootstrap step
- An Entra-aware SQL client for the one-time runtime-user bootstrap step

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

Keep it as a shell-local variable:

```sh
image=<image-ref>
```

### 6. Run the first manual deployment

```sh
az deployment group create \
  --name ade-prod-initial \
  --resource-group rg-ade-prod-canadacentral-002 \
  --template-file infra/main.bicep \
  --parameters @infra/environments/main.prod.parameters.json image="$image"
```

### 7. Grant the deployment identity the minimum Azure RBAC it needs

The deployment identity needs two Azure RBAC grants:

- `Contributor` on the production resource group
- `Managed Identity Operator` on its own user-assigned identity resource so it can attach that identity to the migration job resource during future deployments

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

Grant `Managed Identity Operator` on the deployment identity resource itself:

```sh
az role assignment create \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --assignee-principal-type ServicePrincipal \
  --role "Managed Identity Operator" \
  --scope "${ADE_DEPLOYMENT_IDENTITY_ID}"
```

### 8. Validate the deployment identity grants

List the deployment identity role assignments on the production resource group:

```sh
az role assignment list \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --scope "${ADE_RG_ID}" \
  --output table
```

Confirm the output includes:

- `Contributor`

List the deployment identity role assignments on the deployment identity resource itself:

```sh
az role assignment list \
  --assignee-object-id "${ADE_DEPLOYMENT_IDENTITY_PRINCIPAL_ID}" \
  --scope "${ADE_DEPLOYMENT_IDENTITY_ID}" \
  --output table
```

Confirm the output includes:

- `Managed Identity Operator`

### 9. Grant the SQL logical server identity Microsoft Entra query permissions

The Azure SQL logical server uses a system-assigned managed identity.

That identity must be able to resolve Microsoft Entra principals for `CREATE USER ... FROM EXTERNAL PROVIDER` to work.

Grant it the Directory Readers role, or the documented lower-level Microsoft Graph permissions:

- `User.Read.All`
- `GroupMember.Read.All`
- `Application.Read.All`

This is a tenant-level bootstrap step. It is not handled by Bicep.

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

### 10. Bootstrap runtime SQL access once

Resolve the app managed identity object ID:

```sh
export ADE_APP_PRINCIPAL_ID="$(
  az containerapp show \
    --name ca-ade-prod-canadacentral-002 \
    --resource-group rg-ade-prod-canadacentral-002 \
    --query identity.principalId \
    --output tsv
)"
```

Connect to `sqldb-ade-prod-cc-002` as the deployment identity with your normal Entra-aware SQL client, then run:

```sql
IF NOT EXISTS (
  SELECT 1
  FROM sys.database_principals
  WHERE name = N'ca-ade-prod-canadacentral-002'
)
BEGIN
  CREATE USER [ca-ade-prod-canadacentral-002]
  FROM EXTERNAL PROVIDER
  WITH OBJECT_ID = 'REPLACE_WITH_ADE_APP_PRINCIPAL_ID';
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_role_members AS role_members
  INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
  INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
  WHERE role_principals.name = N'db_datareader'
    AND member_principals.name = N'ca-ade-prod-canadacentral-002'
)
BEGIN
  ALTER ROLE [db_datareader] ADD MEMBER [ca-ade-prod-canadacentral-002];
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_role_members AS role_members
  INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
  INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
  WHERE role_principals.name = N'db_datawriter'
    AND member_principals.name = N'ca-ade-prod-canadacentral-002'
)
BEGIN
  ALTER ROLE [db_datawriter] ADD MEMBER [ca-ade-prod-canadacentral-002];
END;
```

Verify the user exists:

```sql
SELECT name, type_desc
FROM sys.database_principals
WHERE name = N'ca-ade-prod-canadacentral-002';
```

Verify role membership:

```sql
SELECT role_principals.name AS role_name
FROM sys.database_role_members AS role_members
INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
WHERE member_principals.name = N'ca-ade-prod-canadacentral-002';
```

Confirm the output includes:

- `db_datareader`
- `db_datawriter`

### 11. Set the GitHub `production` environment variables

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

### 12. Run the initial migration job manually

The first manual deployment defines the migration job, but it does not execute it.

Run it once manually after the SQL runtime user exists:

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

Verify the deployed app reaches readiness:

```sh
curl -fsS https://<app-fqdn>/api/readyz
```

At steady state, the same start-and-poll sequence is handled by `release_stage` in the deployment pipeline.

## Re-running the deployment

After the bootstrap is complete, future pushes to `main` should use the deployment pipeline.

If you need to reconcile production manually, re-run the template directly with a published image:

```sh
image=<image-ref>

az deployment group create \
  --name ade-prod-reconcile \
  --resource-group rg-ade-prod-canadacentral-002 \
  --template-file infra/main.bicep \
  --parameters @infra/environments/main.prod.parameters.json image="$image"
```

If that deployment changes the migration job or the application image in a way that requires schema changes, start the migration job explicitly afterward.

## Operational notes

- The Container Apps environment is VNet-integrated. Azure does not let you switch an environment from the default network mode to VNet integration in place, so treat the current environment shape as intentional.
- The release workflow deploys the accepted image first and then starts the migration job. Keep schema changes backward-compatible and use expand/contract migrations by default.
- The app readiness endpoint must remain process-level. Do not turn `/api/readyz` into a "latest schema is present" gate.
