param name string
param location string = resourceGroup().location
param tags object = {}
param containerType string = 'PythonLTS'
param cooldownPeriodInSeconds int = 3600
@minValue(5)
@maxValue(600)
param maxConcurrentSessions int = 5
param networkStatus string = 'EgressEnabled'

resource sessionPool 'Microsoft.App/sessionPools@2025-10-02-preview' = {
  name: name
  location: location
  tags: tags
  properties: {
    containerType: containerType
    dynamicPoolConfiguration: {
      lifecycleConfiguration: {
        lifecycleType: 'Timed'
        cooldownPeriodInSeconds: cooldownPeriodInSeconds
      }
    }
    mcpServerSettings: {
      isMcpServerEnabled: true
    }
    poolManagementType: 'Dynamic'
    scaleConfiguration: {
      maxConcurrentSessions: maxConcurrentSessions
    }
    sessionNetworkConfiguration: {
      status: networkStatus
    }
  }
}

output id string = sessionPool.id
output mcpEndpoint string = sessionPool.properties.mcpServerSettings.mcpServerEndpoint
output name string = sessionPool.name
output poolManagementEndpoint string = sessionPool.properties.poolManagementEndpoint
