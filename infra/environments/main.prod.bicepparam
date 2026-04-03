using '../main.bicep'

// This placeholder must be overridden at deploy time with the release candidate image.
param image = 'ghcr.io/clac-ca/ade-platform:unset'
param deploymentManagedIdentityName = 'id-ade-deploy-prod-canadacentral-002'
param githubOrganization = 'clac-ca'
param githubRepository = 'ade'
param githubEnvironmentName = 'production'
param containerAppsEnvironmentName = 'cae-ade-prod-canadacentral-002'
param logAnalyticsWorkspaceName = 'log-ade-prod-canadacentral-002'
param migrationJobName = 'job-ade-migrate-prod-cc-002'
param virtualNetworkName = 'vnet-ade-prod-canadacentral-002'
param sqlServerName = 'sql-ade-prod-cc-002'
param sqlDatabaseName = 'sqldb-ade-prod-cc-002'
param storageAccountName = 'stadeprodcc002'
param keyVaultName = 'kv-ade-prod-cc-002'
param blobContainerName = 'documents'
param tags = {
  environment: 'prod'
}
param appMinReplicas = 0
param appName = 'ca-ade-prod-canadacentral-002'
param sessionPoolName = 'sp-ade-prod-canadacentral-002'
