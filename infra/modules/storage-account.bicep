param accountName string
param blobContainerName string
param corsAllowedOrigins array
param location string
param tags object = {}

resource storageAccount 'Microsoft.Storage/storageAccounts@2024-01-01' = {
  name: accountName
  location: location
  sku: {
    name: 'Standard_LRS'
  }
  kind: 'StorageV2'
  tags: tags
  properties: {
    allowBlobPublicAccess: false
    allowSharedKeyAccess: false
    minimumTlsVersion: 'TLS1_2'
    publicNetworkAccess: 'Enabled'
    supportsHttpsTrafficOnly: true
  }
}

resource blobService 'Microsoft.Storage/storageAccounts/blobServices@2024-01-01' = {
  parent: storageAccount
  name: 'default'
  properties: {
    cors: {
      corsRules: [
        {
          allowedHeaders: [
            'content-type'
            'x-ms-*'
          ]
          allowedMethods: [
            'GET'
            'HEAD'
            'OPTIONS'
            'PUT'
          ]
          allowedOrigins: corsAllowedOrigins
          exposedHeaders: [
            'etag'
            'x-ms-*'
          ]
          maxAgeInSeconds: 3600
        }
      ]
    }
  }
}

resource blobContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2024-01-01' = {
  parent: blobService
  name: blobContainerName
  properties: {
    publicAccess: 'None'
  }
}

output accountId string = storageAccount.id
output blobContainerName string = blobContainer.name
output blobEndpoint string = storageAccount.properties.primaryEndpoints.blob
