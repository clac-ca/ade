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
var storageBlobDataContributorRoleDefinitionId = subscriptionResourceId(
  'Microsoft.Authorization/roleDefinitions',
  'ba92f5b4-2d11-453d-a403-e96b0029c9fe'
)

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

resource storageAccount 'Microsoft.Storage/storageAccounts@2024-01-01' existing = {
  name: storageAccountName
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
    deploymentManagedIdentityClientId: deploymentManagedIdentity.properties.clientId
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

var appSqlConnectionString = 'Data Source=tcp:${sql.outputs.fullyQualifiedDomainName},1433;Initial Catalog=${sql.outputs.databaseName};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'
var migrationSqlConnectionString = 'Data Source=tcp:${sql.outputs.fullyQualifiedDomainName},1433;Initial Catalog=${sql.outputs.databaseName};User ID=${deploymentManagedIdentity.properties.clientId};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'

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
        name: 'AZURE_STORAGEBLOB_RESOURCEENDPOINT'
        value: storage.outputs.blobEndpoint
      }
      {
        name: 'HOST'
        value: '0.0.0.0'
      }
      {
        name: 'PORT'
        value: '8000'
      }
    ]
    externalIngress: true
    image: image
    managedEnvironmentId: platform.outputs.containerAppsEnvironmentId
    maxReplicas: appMaxReplicas
    memory: appMemory
    minReplicas: appMinReplicas
    name: appName
    tags: mergedTags
  }
}

resource blobDataContributorAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(storageAccount.id, appName, storageBlobDataContributorRoleDefinitionId)
  scope: storageAccount
  properties: {
    principalId: app.outputs.principalId
    roleDefinitionId: storageBlobDataContributorRoleDefinitionId
    principalType: 'ServicePrincipal'
  }
}

module migrationJob 'modules/container-app-job.bicep' = {
  name: 'migrationJob'
  params: {
    command: [
      'node'
      'dist/migrate.js'
    ]
    cpu: jobCpu
    deploymentManagedIdentityResourceId: deploymentManagedIdentity.id
    env: [
      {
        name: 'ADE_SQL_RUNTIME_PRINCIPAL_NAME'
        value: appName
      }
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
output sqlServerIdentityPrincipalId string = sql.outputs.serverIdentityPrincipalId
output sqlServerName string = sql.outputs.serverName
output storageAccountName string = storageAccountName
