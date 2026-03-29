targetScope = 'resourceGroup'

@description('Location for all deployed resources.')
param location string = resourceGroup().location

@description('Short prefix used for shared infrastructure resource names.')
param prefix string = 'ade'

@description('Tags applied to all resources.')
param tags object = {}

@description('Container image reference for the deployed ADE app and migration job.')
param image string

@description('Name for the deployment user-assigned managed identity.')
param deploymentManagedIdentityName string

@description('GitHub organization used in the deployment-identity federated credential subject.')
param githubOrganization string

@description('GitHub repository used in the deployment-identity federated credential subject.')
param githubRepository string

@description('GitHub environment name used in the deployment-identity federated credential subject.')
param githubEnvironmentName string

@description('Name for the Log Analytics workspace.')
param logAnalyticsWorkspaceName string = '${prefix}-logs'

@description('Name for the Container Apps environment.')
param containerAppsEnvironmentName string = '${prefix}-env'

@description('Name for the public ADE container app.')
param appName string = prefix

@description('Name for the manual migration job.')
param migrationJobName string = '${prefix}-migrate'

@description('Name for the virtual network.')
param virtualNetworkName string = '${prefix}-vnet'

@description('Name for the Container Apps infrastructure subnet.')
param containerAppsSubnetName string = 'aca-infra'

@description('Name for the Azure SQL logical server.')
param sqlServerName string = '${prefix}-sql'

@description('Name for the Azure SQL database.')
param sqlDatabaseName string = 'sqldb-${prefix}'

@description('Name for the Azure Storage account.')
param storageAccountName string

@description('Name for the Blob container.')
param blobContainerName string = 'documents'

@description('Name for the Azure Container Apps session pool.')
param sessionPoolName string = '${prefix}-sessions'

@description('Secret used to derive deterministic ADE runtime session identifiers.')
@secure()
param runtimeSessionSecret string

@description('JSON array mapping workspace/config pairs to config wheel paths inside the app container.')
param configTargets string = string([
  {
    workspaceId: 'workspace-a'
    configVersionId: 'config-v1'
    wheelPath: '/app/python/ade_config.whl'
  }
  {
    workspaceId: 'workspace-b'
    configVersionId: 'config-v2'
    wheelPath: '/app/python/ade_config.whl'
  }
])

@description('CPU allocation for the ADE container app.')
param appCpu string = '0.25'

@description('Memory allocation for the ADE container app.')
param appMemory string = '0.5Gi'

@description('Minimum replica count for the ADE container app.')
param appMinReplicas int = 1

@description('Maximum replica count for the ADE container app.')
param appMaxReplicas int = 1

@description('CPU allocation for the migration job.')
param jobCpu string = '0.25'

@description('Memory allocation for the migration job.')
param jobMemory string = '0.5Gi'

@description('Address prefix for the ADE virtual network.')
param virtualNetworkAddressPrefix string = '10.42.0.0/16'

@description('Address prefix for the Container Apps subnet.')
param containerAppsSubnetAddressPrefix string = '10.42.0.0/27'

var mergedTags = union({
  project: 'ade'
}, tags)
var githubOidcSubject = 'repo:${githubOrganization}/${githubRepository}:environment:${githubEnvironmentName}'
var sessionExecutorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '0fb8eba5-a2bb-4abe-b1c1-49dfad359bb0')
var storageBlobDataContributorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'ba92f5b4-2d11-453d-a403-e96b0029c9fe')

resource deploymentManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: deploymentManagedIdentityName
  location: location
  tags: mergedTags
}

resource deploymentManagedIdentityFederatedCredential 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2024-11-30' = {
  parent: deploymentManagedIdentity
  name: 'github-${githubEnvironmentName}'
  properties: {
    audiences: [
      'api://AzureADTokenExchange'
    ]
    issuer: 'https://token.actions.githubusercontent.com'
    subject: githubOidcSubject
  }
}

module network 'modules/network.bicep' = {
  name: 'adeNetwork'
  params: {
    containerAppsSubnetAddressPrefix: containerAppsSubnetAddressPrefix
    containerAppsSubnetName: containerAppsSubnetName
    location: location
    tags: mergedTags
    virtualNetworkAddressPrefix: virtualNetworkAddressPrefix
    virtualNetworkName: virtualNetworkName
  }
}

module platform 'modules/container-app-platform.bicep' = {
  name: 'containerAppPlatform'
  params: {
    containerAppsEnvironmentName: containerAppsEnvironmentName
    infrastructureSubnetId: network.outputs.containerAppsSubnetId
    location: location
    logAnalyticsWorkspaceName: logAnalyticsWorkspaceName
    tags: mergedTags
  }
}

module sql 'modules/sql-database.bicep' = {
  name: 'sqlDatabase'
  params: {
    databaseName: sqlDatabaseName
    deploymentManagedIdentityName: deploymentManagedIdentity.name
    deploymentManagedIdentityPrincipalId: deploymentManagedIdentity.properties.principalId
    location: location
    serverName: sqlServerName
    tags: mergedTags
    virtualNetworkSubnetId: network.outputs.containerAppsSubnetId
  }
}

module storage 'modules/storage-account.bicep' = {
  name: 'storageAccount'
  params: {
    accountName: storageAccountName
    blobContainerName: blobContainerName
    location: location
    tags: mergedTags
    virtualNetworkSubnetId: network.outputs.containerAppsSubnetId
  }
}

module sessionPool 'modules/session-pool.bicep' = {
  name: 'sessionPool'
  params: {
    location: location
    name: sessionPoolName
    tags: mergedTags
  }
}

resource sessionPoolResource 'Microsoft.App/sessionPools@2025-10-02-preview' existing = {
  name: sessionPoolName
}

resource storageAccountResource 'Microsoft.Storage/storageAccounts@2024-01-01' existing = {
  name: storageAccountName
}

var appSqlConnectionString = 'Data Source=tcp:${sql.outputs.fullyQualifiedDomainName},1433;Initial Catalog=${sql.outputs.databaseName};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'
var migrationSqlConnectionString = 'Data Source=tcp:${sql.outputs.fullyQualifiedDomainName},1433;Initial Catalog=${sql.outputs.databaseName};User ID=${deploymentManagedIdentity.properties.clientId};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'
var appPublicUrl = 'https://${appName}.${platform.outputs.defaultDomain}'

module app 'modules/container-app.bicep' = {
  name: 'appContainerApp'
  params: {
    containerPort: 8000
    cpu: appCpu
    env: [
      {
        name: 'AZURE_SQL_CONNECTIONSTRING'
        value: appSqlConnectionString
      }
      {
        name: 'ADE_SESSION_SECRET'
        secretRef: 'ade-runtime-session-secret'
      }
      {
        name: 'ADE_CONFIG_TARGETS'
        value: configTargets
      }
      {
        name: 'ADE_SESSION_POOL_MANAGEMENT_ENDPOINT'
        value: sessionPool.outputs.poolManagementEndpoint
      }
      {
        name: 'ADE_APP_URL'
        value: appPublicUrl
      }
      {
        name: 'ADE_BLOB_ACCOUNT_URL'
        value: storage.outputs.blobEndpoint
      }
      {
        name: 'ADE_BLOB_CONTAINER'
        value: storage.outputs.blobContainerName
      }
    ]
    probes: [
      {
        type: 'Startup'
        httpGet: {
          path: '/api/healthz'
          port: 8000
          scheme: 'HTTP'
        }
        initialDelaySeconds: 1
        periodSeconds: 5
        timeoutSeconds: 3
        failureThreshold: 24
        successThreshold: 1
      }
      {
        type: 'Readiness'
        httpGet: {
          path: '/api/readyz'
          port: 8000
          scheme: 'HTTP'
        }
        initialDelaySeconds: 1
        periodSeconds: 5
        timeoutSeconds: 3
        failureThreshold: 3
        successThreshold: 1
      }
      {
        type: 'Liveness'
        httpGet: {
          path: '/api/healthz'
          port: 8000
          scheme: 'HTTP'
        }
        initialDelaySeconds: 10
        periodSeconds: 30
        timeoutSeconds: 3
        failureThreshold: 3
        successThreshold: 1
      }
    ]
    externalIngress: true
    image: image
    managedEnvironmentId: platform.outputs.containerAppsEnvironmentId
    maxReplicas: appMaxReplicas
    memory: appMemory
    minReplicas: appMinReplicas
    name: appName
    secrets: [
      {
        name: 'ade-runtime-session-secret'
        value: runtimeSessionSecret
      }
    ]
    tags: mergedTags
  }
}

resource appSessionPoolExecutorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(sessionPoolResource.id, appName, sessionExecutorRoleDefinitionId)
  scope: sessionPoolResource
  properties: {
    principalId: app.outputs.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: sessionExecutorRoleDefinitionId
  }
}

resource appStorageBlobDataContributorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(storageAccountResource.id, appName, storageBlobDataContributorRoleDefinitionId)
  scope: storageAccountResource
  properties: {
    principalId: app.outputs.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: storageBlobDataContributorRoleDefinitionId
  }
}

module migrationJob 'modules/container-app-job.bicep' = {
  name: 'migrationJob'
  params: {
    command: [
      './bin/ade-migrate'
    ]
    cpu: jobCpu
    deploymentManagedIdentityResourceId: deploymentManagedIdentity.id
    env: [
      {
        name: 'AZURE_SQL_CONNECTIONSTRING'
        value: migrationSqlConnectionString
      }
    ]
    image: image
    managedEnvironmentId: platform.outputs.containerAppsEnvironmentId
    memory: jobMemory
    name: migrationJobName
    tags: mergedTags
  }
}

output appFqdn string = app.outputs.fqdn
output appName string = appName
output appPrincipalId string = app.outputs.principalId
output appUrl string = app.outputs.url
output containerAppsEnvironmentId string = platform.outputs.containerAppsEnvironmentId
output deploymentManagedIdentityClientId string = deploymentManagedIdentity.properties.clientId
output deploymentManagedIdentityName string = deploymentManagedIdentity.name
output deploymentManagedIdentityPrincipalId string = deploymentManagedIdentity.properties.principalId
output migrationJobName string = migrationJobName
output sqlDatabaseName string = sql.outputs.databaseName
output sessionPoolId string = sessionPool.outputs.id
output sessionPoolName string = sessionPool.outputs.name
output sessionPoolPoolManagementEndpoint string = sessionPool.outputs.poolManagementEndpoint
output sqlServerIdentityPrincipalId string = sql.outputs.serverIdentityPrincipalId
output sqlServerName string = sql.outputs.serverName
output storageAccountName string = storageAccountName
