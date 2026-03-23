targetScope = 'resourceGroup'

@description('Location for all deployed resources.')
param location string = resourceGroup().location

@description('Short prefix used for shared infrastructure resource names.')
param prefix string = 'ade'

@description('Tags applied to all resources.')
param tags object = {}

@description('Container image reference for the deployed ADE app.')
param image string

@description('Name for the Log Analytics workspace.')
param logAnalyticsWorkspaceName string = '${prefix}-logs'

@description('Name for the Container Apps environment.')
param containerAppsEnvironmentName string = '${prefix}-env'

@description('User-assigned managed identity resource ID attached to the ADE app for runtime access.')
param runtimeManagedIdentityResourceId string = ''

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
    runtimeManagedIdentityResourceId: runtimeManagedIdentityResourceId
    tags: mergedTags
  }
}

output containerAppsEnvironmentId string = platform.outputs.containerAppsEnvironmentId
output appName string = appName
output appFqdn string = app.outputs.fqdn
output appUrl string = app.outputs.url
