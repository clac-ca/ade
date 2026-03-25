param virtualNetworkName string
param containerAppsSubnetName string
param location string
param tags object = {}
param virtualNetworkAddressPrefix string = '10.42.0.0/16'
param containerAppsSubnetAddressPrefix string = '10.42.0.0/27'

resource virtualNetwork 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: virtualNetworkName
  location: location
  tags: tags
  properties: {
    addressSpace: {
      addressPrefixes: [
        virtualNetworkAddressPrefix
      ]
    }
  }
}

resource containerAppsSubnet 'Microsoft.Network/virtualNetworks/subnets@2024-05-01' = {
  parent: virtualNetwork
  name: containerAppsSubnetName
  properties: {
    addressPrefix: containerAppsSubnetAddressPrefix
    delegations: [
      {
        name: 'containerAppsDelegation'
        properties: {
          serviceName: 'Microsoft.App/environments'
        }
      }
    ]
    serviceEndpoints: [
      {
        service: 'Microsoft.Sql'
        locations: [
          location
        ]
      }
      {
        service: 'Microsoft.Storage.Global'
        locations: [
          location
        ]
      }
    ]
  }
}

output containerAppsSubnetId string = containerAppsSubnet.id
