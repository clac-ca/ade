targetScope = 'resourceGroup'

@description('Location for all deployed resources.')
param location string = resourceGroup().location

@description('Short prefix used for shared infrastructure resource names.')
param prefix string = 'ade'

@description('Tags applied to all resources.')
param tags object = {}

@description('Container image reference for the deployed ADE app.')
param image string

@description('Name for the deployment user-assigned managed identity.')
param deploymentManagedIdentityName string

@description('Name for the runtime user-assigned managed identity.')
param runtimeManagedIdentityName string

@description('Name for the Key Vault.')
param keyVaultName string

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

@description('CPU allocation for the ADE container app.')
param appCpu string = '0.25'

@description('Memory allocation for the ADE container app.')
param appMemory string = '0.5Gi'

@description('Minimum replica count for the ADE container app.')
param appMinReplicas int = 1

@description('Maximum replica count for the ADE container app.')
param appMaxReplicas int = 1

var mergedTags = union({
  project: 'ade'
}, tags)
var githubOidcSubject = 'repo:${githubOrganization}/${githubRepository}:environment:${githubEnvironmentName}'

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

resource runtimeManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: runtimeManagedIdentityName
  location: location
  tags: mergedTags
}

resource keyVault 'Microsoft.KeyVault/vaults@2023-07-01' = {
  name: keyVaultName
  location: location
  tags: mergedTags
  properties: {
    enableRbacAuthorization: true
    publicNetworkAccess: 'Enabled'
    sku: {
      family: 'A'
      name: 'standard'
    }
    tenantId: tenant().tenantId
  }
}

module platform 'modules/container-app-platform.bicep' = {
  name: 'containerAppPlatform'
  params: {
    containerAppsEnvironmentName: containerAppsEnvironmentName
    location: location
    logAnalyticsWorkspaceName: logAnalyticsWorkspaceName
    tags: mergedTags
  }
}

module app 'modules/container-app.bicep' = {
  name: 'appContainerApp'
  params: {
    containerPort: 8000
    cpu: appCpu
    env: [
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
    runtimeManagedIdentityResourceId: runtimeManagedIdentity.id
    tags: mergedTags
  }
}

output containerAppsEnvironmentId string = platform.outputs.containerAppsEnvironmentId
output appName string = appName
output appFqdn string = app.outputs.fqdn
output appUrl string = app.outputs.url
output deploymentManagedIdentityClientId string = deploymentManagedIdentity.properties.clientId
output deploymentManagedIdentityName string = deploymentManagedIdentity.name
output deploymentManagedIdentityPrincipalId string = deploymentManagedIdentity.properties.principalId
output keyVaultName string = keyVault.name
output runtimeManagedIdentityName string = runtimeManagedIdentity.name
output runtimeManagedIdentityPrincipalId string = runtimeManagedIdentity.properties.principalId
output runtimeManagedIdentityResourceId string = runtimeManagedIdentity.id
