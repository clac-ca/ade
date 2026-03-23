targetScope = 'resourceGroup'

@description('Location for all deployed resources.')
param location string = resourceGroup().location

@description('Short prefix used for shared infrastructure resource names.')
param prefix string = 'ade'

@description('Tags applied to all resources.')
param tags object = {}

@description('Container image reference for the deployed ADE app.')
param image string

@description('Container registry server used by the deployed apps.')
param registryServer string = ''

@description('Container registry username used by the deployed apps.')
param registryUsername string = ''

@description('Container registry password or token used by the deployed apps.')
@secure()
param registryPassword string = ''

@description('Name for the Log Analytics workspace.')
param logAnalyticsWorkspaceName string = '${prefix}-logs'

@description('Name for the Container Apps environment.')
param containerAppsEnvironmentName string = '${prefix}-env'

@description('Name for the public ADE container app.')
param webAppName string = 'web'

@description('CPU allocation for the ADE container app.')
param webCpu string = '0.25'

@description('Memory allocation for the ADE container app.')
param webMemory string = '0.5Gi'

@description('Minimum replica count for the ADE container app.')
param webMinReplicas int = 1

@description('Maximum replica count for the ADE container app.')
param webMaxReplicas int = 1

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

module web 'modules/container-app.bicep' = {
  name: 'webContainerApp'
  params: {
    containerPort: 8000
    cpu: webCpu
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
    maxReplicas: webMaxReplicas
    memory: webMemory
    minReplicas: webMinReplicas
    name: webAppName
    registryPassword: registryPassword
    registryServer: registryServer
    registryUsername: registryUsername
    tags: mergedTags
  }
}

output containerAppsEnvironmentId string = platform.outputs.containerAppsEnvironmentId
output webAppName string = webAppName
output webFqdn string = web.outputs.fqdn
output webUrl string = web.outputs.url
