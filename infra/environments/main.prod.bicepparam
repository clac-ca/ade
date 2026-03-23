using '../main.bicep'

param deploymentManagedIdentityName = 'id-ade-deploy-prod-canadacentral-002'
param runtimeManagedIdentityName = 'id-ade-runtime-prod-canadacentral-002'
param keyVaultName = 'kv-ade-prod-cc-002'
param githubOrganization = 'clac-ca'
param githubRepository = 'ade'
param githubEnvironmentName = 'production'
param containerAppsEnvironmentName = 'cae-ade-prod-canadacentral-002'
param logAnalyticsWorkspaceName = 'log-ade-prod-canadacentral-002'
param tags = {
  environment: 'prod'
}
param image = readEnvironmentVariable('ADE_IMAGE')
param appMinReplicas = 0
param appName = 'ca-ade-prod-canadacentral-002'
