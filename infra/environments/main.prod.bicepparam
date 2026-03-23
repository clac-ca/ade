using '../main.bicep'

param containerAppsEnvironmentName = 'cae-ade-prod-canadacentral-002'
param logAnalyticsWorkspaceName = 'log-ade-prod-canadacentral-002'
param tags = {
  environment: 'prod'
}
param runtimeManagedIdentityResourceId = readEnvironmentVariable('ADE_RUNTIME_MANAGED_IDENTITY_RESOURCE_ID')
param appMinReplicas = 0
param appName = 'ca-ade-prod-canadacentral-002'
param image = readEnvironmentVariable('ADE_IMAGE')
