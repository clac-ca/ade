targetScope = 'resourceGroup'

@description('Location for all deployed resources.')
param location string = resourceGroup().location

@description('Short prefix used for shared infrastructure resource names.')
param prefix string = 'ade'

@description('Tags applied to all resources.')
param tags object = {}

@description('Container image reference for the web app.')
param webImage string

@description('Container image reference for the API app.')
param apiImage string

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

@description('Name for the public web container app.')
param webAppName string = 'web'

@description('Name for the internal API container app. Keep this as `api` while apps/web/nginx.conf proxies to `http://api:8001`.')
param apiAppName string = 'api'

@description('CPU allocation for the web container app.')
param webCpu string = '0.25'

@description('Memory allocation for the web container app.')
param webMemory string = '0.5Gi'

@description('CPU allocation for the API container app.')
param apiCpu string = '0.25'

@description('Memory allocation for the API container app.')
param apiMemory string = '0.5Gi'

@description('Minimum replica count for the web container app.')
param webMinReplicas int = 1

@description('Maximum replica count for the web container app.')
param webMaxReplicas int = 1

@description('Minimum replica count for the API container app.')
param apiMinReplicas int = 1

@description('Maximum replica count for the API container app.')
param apiMaxReplicas int = 1

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
    containerPort: 80
    cpu: webCpu
    env: []
    externalIngress: true
    image: webImage
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

module api 'modules/container-app.bicep' = {
  name: 'apiContainerApp'
  params: {
    containerPort: 8001
    cpu: apiCpu
    env: [
      {
        name: 'ADE_API_HOST'
        value: '0.0.0.0'
      }
      {
        name: 'ADE_API_PORT'
        value: '8001'
      }
    ]
    externalIngress: false
    image: apiImage
    managedEnvironmentId: platform.outputs.containerAppsEnvironmentId
    maxReplicas: apiMaxReplicas
    memory: apiMemory
    minReplicas: apiMinReplicas
    name: apiAppName
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
output apiAppName string = apiAppName
output apiFqdn string = api.outputs.fqdn
