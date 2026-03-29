param name string
param location string = resourceGroup().location
param tags object = {}
param cooldownPeriodInSeconds int = 3600
@minValue(5)
@maxValue(600)
param maxConcurrentSessions int = 5

resource sessionPool 'Microsoft.App/sessionPools@2025-10-02-preview' = {
  name: name
  location: location
  tags: tags
  properties: {
    containerType: 'PythonLTS'
    dynamicPoolConfiguration: {
      lifecycleConfiguration: {
        lifecycleType: 'Timed'
        cooldownPeriodInSeconds: cooldownPeriodInSeconds
      }
    }
    poolManagementType: 'Dynamic'
    scaleConfiguration: {
      maxConcurrentSessions: maxConcurrentSessions
    }
    sessionNetworkConfiguration: {
      status: 'EgressEnabled'
    }
  }
}

output id string = sessionPool.id
output name string = sessionPool.name
output poolManagementEndpoint string = sessionPool.properties.poolManagementEndpoint
