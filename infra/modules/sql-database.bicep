param serverName string
param databaseName string
param deploymentManagedIdentityName string
param deploymentManagedIdentityPrincipalId string
param virtualNetworkSubnetId string
param location string
param tags object = {}
param skuName string = 'GP_S_Gen5'
param skuTier string = 'GeneralPurpose'
param skuFamily string = 'Gen5'
param skuCapacity int = 2
param autoPauseDelay int = 60
var minCapacity = json('0.5')

resource sqlServer 'Microsoft.Sql/servers@2023-08-01' = {
  name: serverName
  location: location
  identity: {
    type: 'SystemAssigned'
  }
  tags: tags
  properties: {
    minimalTlsVersion: '1.2'
    publicNetworkAccess: 'Enabled'
    version: '12.0'
  }
}

resource sqlAdministrator 'Microsoft.Sql/servers/administrators@2023-08-01' = {
  parent: sqlServer
  name: 'ActiveDirectory'
  properties: {
    administratorType: 'ActiveDirectory'
    login: deploymentManagedIdentityName
    sid: deploymentManagedIdentityPrincipalId
    tenantId: tenant().tenantId
  }
}

resource sqlDatabase 'Microsoft.Sql/servers/databases@2023-08-01' = {
  parent: sqlServer
  name: databaseName
  location: location
  sku: {
    name: skuName
    tier: skuTier
    family: skuFamily
    capacity: skuCapacity
  }
  tags: tags
  properties: {
    autoPauseDelay: autoPauseDelay
    minCapacity: minCapacity
  }
}

resource sqlEntraOnlyAuth 'Microsoft.Sql/servers/azureADOnlyAuthentications@2023-08-01' = {
  parent: sqlServer
  name: 'Default'
  properties: {
    azureADOnlyAuthentication: true
  }
}

resource virtualNetworkRule 'Microsoft.Sql/servers/virtualNetworkRules@2023-08-01' = {
  parent: sqlServer
  name: 'aca'
  properties: {
    ignoreMissingVnetServiceEndpoint: false
    virtualNetworkSubnetId: virtualNetworkSubnetId
  }
}

output databaseName string = sqlDatabase.name
output fullyQualifiedDomainName string = sqlServer.properties.fullyQualifiedDomainName
output serverIdentityPrincipalId string = sqlServer.identity.principalId
output serverName string = sqlServer.name
