# ADE Infrastructure

## Overview

Production uses two Bicep templates:

- [`bootstrap.bicep`](bootstrap.bicep): one-time Azure prerequisites
- [`main.bicep`](main.bicep): normal deploys

ADE manages two user-assigned identities directly:

| Identity            | Used by                       | Purpose                                    |
| ------------------- | ----------------------------- | ------------------------------------------ |
| Deployment identity | GitHub Actions, migration job | Azure deploys, SQL migrations              |
| App identity        | Container App                 | Key Vault, SQL, Blob Storage, session pool |

```sh
RESOURCE_GROUP=rg-ade-prod-canadacentral-002
LOCATION=canadacentral
KEY_VAULT_NAME=kv-ade-prod-cc-002
DEPLOYMENT_IDENTITY_NAME=id-ade-deploy-prod-canadacentral-002
APP_IDENTITY_NAME=id-ade-app-prod-canadacentral-002
APP_NAME=ca-ade-prod-canadacentral-002
MIGRATION_JOB_NAME=job-ade-migrate-prod-cc-002
SQL_SERVER_NAME=sql-ade-prod-cc-002
SQL_DATABASE_NAME=sqldb-ade-prod-cc-002
SQL_BOOTSTRAP_FIREWALL_RULE_NAME=ade-bootstrap-operator-ip
```

## Once Per Subscription Or Tenant

### Sign in and select the subscription

```sh
az login
az account set --subscription <subscription-id>
az account show --query '{tenantId:tenantId,subscriptionId:id,name:name}' --output table
```

### Register the required resource providers

```sh
az provider register --namespace Microsoft.App
az provider register --namespace Microsoft.KeyVault
az provider register --namespace Microsoft.ManagedIdentity
az provider register --namespace Microsoft.Network
az provider register --namespace Microsoft.OperationalInsights
az provider register --namespace Microsoft.Sql
az provider register --namespace Microsoft.Storage
```

## Once Per Environment

### Create the resource group

```sh
az group create \
  --name "$RESOURCE_GROUP" \
  --location "$LOCATION"
```

### Choose the release image

Use an immutable image digest:

```text
ghcr.io/clac-ca/ade-platform@sha256:<digest>
```

```sh
IMAGE=<image-ref>
```

### Run the bootstrap template

```sh
az deployment group create \
  --name ade-prod-bootstrap \
  --resource-group "$RESOURCE_GROUP" \
  --template-file infra/bootstrap.bicep \
  --output none
```

This creates the deployment identity, the app identity, the GitHub OIDC credential, the Key Vault, and the required RBAC assignments.

### Seed the sandbox secret in Key Vault

The operator running this step needs a role that can write Key Vault secrets, such as `Key Vault Secrets Officer` on the vault.

```sh
SANDBOX_ENVIRONMENT_SECRET="$(openssl rand -hex 32)"

az keyvault secret set \
  --vault-name "$KEY_VAULT_NAME" \
  --name ade-sandbox-environment-secret \
  --value "$SANDBOX_ENVIRONMENT_SECRET" \
  --output none

unset SANDBOX_ENVIRONMENT_SECRET
```

This writes the secret once. Later deploys only reference the existing Key Vault value.

### Set the GitHub `production` environment variables

Set the deployment identity client ID for `azure/login`:

```sh
gh variable set AZURE_CLIENT_ID \
  --env production \
  --repo clac-ca/ade \
  --body "$(
    az identity show \
      --name "$DEPLOYMENT_IDENTITY_NAME" \
      --resource-group "$RESOURCE_GROUP" \
      --query clientId \
      --output tsv
  )"
```

Set the other required variables:

```sh
gh variable set AZURE_TENANT_ID --env production --repo clac-ca/ade --body "<tenant-id>"
gh variable set AZURE_SUBSCRIPTION_ID --env production --repo clac-ca/ade --body "<subscription-id>"
gh variable set AZURE_RESOURCE_GROUP --env production --repo clac-ca/ade --body "$RESOURCE_GROUP"
```

### Run the main template

```sh
APP_URL="$(
  az deployment group create \
    --name ade-prod-main \
    --resource-group "$RESOURCE_GROUP" \
    --template-file infra/main.bicep \
    --parameters image="$IMAGE" \
    --query properties.outputs.appUrl.value \
    --output tsv
)"
```

This creates the network, SQL, storage, session pool, Container App, and migration job. The app attaches only the app identity and reads `ADE_SANDBOX_ENVIRONMENT_SECRET` from Key Vault.

### Set the SQL server Entra admin once

For the manual SQL bootstrap, set the logical server's Microsoft Entra admin to the operator running the one-time setup. If you want shared operator access, use a group instead of your user.

```sh
SQL_ADMIN_DISPLAY_NAME="$(az account show --query user.name --output tsv)"
SQL_ADMIN_OBJECT_ID="$(az ad signed-in-user show --query id --output tsv)"

az sql server ad-admin create \
  --resource-group "$RESOURCE_GROUP" \
  --server "$SQL_SERVER_NAME" \
  --display-name "$SQL_ADMIN_DISPLAY_NAME" \
  --object-id "$SQL_ADMIN_OBJECT_ID" \
  --output none

az sql server ad-only-auth enable \
  --resource-group "$RESOURCE_GROUP" \
  --name "$SQL_SERVER_NAME" \
  --output none
```

### Open your current IP long enough to run the SQL bootstrap

```sh
SQL_OPERATOR_IP="$(curl -fsS https://api.ipify.org)"

az sql server firewall-rule create \
  --resource-group "$RESOURCE_GROUP" \
  --server "$SQL_SERVER_NAME" \
  --name "$SQL_BOOTSTRAP_FIREWALL_RULE_NAME" \
  --start-ip-address "$SQL_OPERATOR_IP" \
  --end-ip-address "$SQL_OPERATOR_IP" \
  --output none
```

### Create the database users once

ADE uses Azure SQL Database's documented "without validation" user-creation path, so this bootstrap does not require Microsoft Graph permissions on the SQL server. The reference syntax is in [CREATE USER (Transact-SQL)](https://learn.microsoft.com/en-us/sql/t-sql/statements/create-user-transact-sql?tabs=sqlserver&view=sql-server-2017).

Resolve the app and deployment identity client IDs:

```sh
ADE_APP_UAMI_CLIENT_ID="$(
  az identity show \
    --name "$APP_IDENTITY_NAME" \
    --resource-group "$RESOURCE_GROUP" \
    --query clientId \
    --output tsv
)"

ADE_DEPLOY_UAMI_CLIENT_ID="$(
  az identity show \
    --name "$DEPLOYMENT_IDENTITY_NAME" \
    --resource-group "$RESOURCE_GROUP" \
    --query clientId \
    --output tsv
)"

cat <<SQL >/tmp/ade-bootstrap.sql
DECLARE @appClientId UNIQUEIDENTIFIER = '${ADE_APP_UAMI_CLIENT_ID}';
DECLARE @appSid NVARCHAR(MAX) = CONVERT(VARCHAR(MAX), CONVERT(VARBINARY(16), @appClientId), 1);
DECLARE @deployClientId UNIQUEIDENTIFIER = '${ADE_DEPLOY_UAMI_CLIENT_ID}';
DECLARE @deploySid NVARCHAR(MAX) = CONVERT(VARCHAR(MAX), CONVERT(VARBINARY(16), @deployClientId), 1);

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_principals
  WHERE name = N'id-ade-app-prod-canadacentral-002'
)
BEGIN
  EXEC(N'CREATE USER [id-ade-app-prod-canadacentral-002] WITH SID = ' + @appSid + ', TYPE = E;');
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_role_members AS role_members
  INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
  INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
  WHERE role_principals.name = N'db_datareader'
    AND member_principals.name = N'id-ade-app-prod-canadacentral-002'
)
BEGIN
  ALTER ROLE [db_datareader] ADD MEMBER [id-ade-app-prod-canadacentral-002];
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_role_members AS role_members
  INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
  INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
  WHERE role_principals.name = N'db_datawriter'
    AND member_principals.name = N'id-ade-app-prod-canadacentral-002'
)
BEGIN
  ALTER ROLE [db_datawriter] ADD MEMBER [id-ade-app-prod-canadacentral-002];
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_principals
  WHERE name = N'id-ade-deploy-prod-canadacentral-002'
)
BEGIN
  EXEC(N'CREATE USER [id-ade-deploy-prod-canadacentral-002] WITH SID = ' + @deploySid + ', TYPE = E;');
END;

IF NOT EXISTS (
  SELECT 1
  FROM sys.database_role_members AS role_members
  INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
  INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
  WHERE role_principals.name = N'db_owner'
    AND member_principals.name = N'id-ade-deploy-prod-canadacentral-002'
)
BEGIN
  ALTER ROLE [db_owner] ADD MEMBER [id-ade-deploy-prod-canadacentral-002];
END;
SQL

sqlcmd \
  -S "${SQL_SERVER_NAME}.database.windows.net" \
  -d "$SQL_DATABASE_NAME" \
  -G \
  -C \
  -i /tmp/ade-bootstrap.sql

rm /tmp/ade-bootstrap.sql

az sql server firewall-rule delete \
  --resource-group "$RESOURCE_GROUP" \
  --server "$SQL_SERVER_NAME" \
  --name "$SQL_BOOTSTRAP_FIREWALL_RULE_NAME" \
  --yes
```

### Run the first migration and verify readiness

```sh
az containerapp job start \
  --name "$MIGRATION_JOB_NAME" \
  --resource-group "$RESOURCE_GROUP"
```

```sh
curl -fsS "${APP_URL}/api/readyz"
```

## Every Deploy

### Deploy the new image

```sh
IMAGE=<image-ref>

az deployment group create \
  --name ade-prod-update \
  --resource-group "$RESOURCE_GROUP" \
  --template-file infra/main.bicep \
  --parameters image="$IMAGE" \
  --output none
```

### Run migrations when the release changes the schema

```sh
az containerapp job start \
  --name "$MIGRATION_JOB_NAME" \
  --resource-group "$RESOURCE_GROUP"
```

### Verify readiness

```sh
APP_URL="$(
  az containerapp show \
    --name "$APP_NAME" \
    --resource-group "$RESOURCE_GROUP" \
    --query properties.configuration.ingress.fqdn \
    --output tsv
)"

curl -fsS "https://${APP_URL}/api/readyz"
```

## Steady-State Rules

- `bootstrap.bicep` never writes secrets.
- `main.bicep` never writes secrets.
- SQL server Entra admin and database user creation are one-time operator bootstrap steps.
- Migrations run separately through the Container Apps job.
- Normal releases only deploy `infra/main.bicep` and start the migration job when needed.
