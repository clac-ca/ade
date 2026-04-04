targetScope = 'resourceGroup'

param image string

var location = resourceGroup().location
var mergedTags = {
  project: 'ade'
  environment: 'prod'
}
var deploymentManagedIdentityName = 'id-ade-deploy-prod-canadacentral-002'
var appManagedIdentityName = 'id-ade-app-prod-canadacentral-002'
var logAnalyticsWorkspaceName = 'log-ade-prod-canadacentral-002'
var containerAppsEnvironmentName = 'cae-ade-prod-canadacentral-002'
var appName = 'ca-ade-prod-canadacentral-002'
var migrationJobName = 'job-ade-migrate-prod-cc-002'
var virtualNetworkName = 'vnet-ade-prod-canadacentral-002'
var containerAppsSubnetName = 'aca-infra'
var sqlServerName = 'sql-ade-prod-cc-002'
var sqlDatabaseName = 'sqldb-ade-prod-cc-002'
var storageAccountName = 'stadeprodcc002'
var keyVaultName = 'kv-ade-prod-cc-002'
var blobContainerName = 'documents'
var sessionPoolName = 'spadeprodcc002'
var appCpu = '0.25'
var appMemory = '0.5Gi'
var appMinReplicas = 0
var appMaxReplicas = 1
var jobCpu = '0.25'
var jobMemory = '0.5Gi'
var virtualNetworkAddressPrefix = '10.42.0.0/16'
var containerAppsSubnetAddressPrefix = '10.42.0.0/27'
var sessionExecutorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '0fb8eba5-a2bb-4abe-b1c1-49dfad359bb0')
var storageBlobDataContributorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'ba92f5b4-2d11-453d-a403-e96b0029c9fe')
var sandboxEnvironmentSecretName = 'ade-sandbox-environment-secret'

resource deploymentManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' existing = {
  name: deploymentManagedIdentityName
}

resource appManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' existing = {
  name: appManagedIdentityName
}

resource keyVault 'Microsoft.KeyVault/vaults@2023-07-01' existing = {
  name: keyVaultName
}

var sandboxEnvironmentSecretKeyVaultUrl = '${keyVault.properties.vaultUri}secrets/${sandboxEnvironmentSecretName}'

resource virtualNetwork 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: virtualNetworkName
  location: location
  tags: mergedTags
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
    ]
  }
}

resource workspace 'Microsoft.OperationalInsights/workspaces@2022-10-01' = {
  name: logAnalyticsWorkspaceName
  location: location
  tags: mergedTags
  properties: {
    features: {
      disableLocalAuth: false
      enableLogAccessUsingOnlyResourcePermissions: true
    }
    publicNetworkAccessForIngestion: 'Enabled'
    publicNetworkAccessForQuery: 'Enabled'
    retentionInDays: 30
    sku: {
      name: 'PerGB2018'
    }
    workspaceCapping: {
      dailyQuotaGb: -1
    }
  }
}

resource containerAppsEnvironment 'Microsoft.App/managedEnvironments@2024-03-01' = {
  name: containerAppsEnvironmentName
  location: location
  tags: mergedTags
  properties: {
    appLogsConfiguration: {
      destination: 'log-analytics'
      logAnalyticsConfiguration: {
        customerId: workspace.properties.customerId
        sharedKey: workspace.listKeys().primarySharedKey
      }
    }
    vnetConfiguration: {
      infrastructureSubnetId: containerAppsSubnet.id
      internal: false
    }
    workloadProfiles: [
      {
        name: 'Consumption'
        workloadProfileType: 'Consumption'
      }
    ]
  }
}

resource sqlServer 'Microsoft.Sql/servers@2023-02-01-preview' = {
  name: sqlServerName
  location: location
  tags: mergedTags
  properties: {
    minimalTlsVersion: '1.2'
    publicNetworkAccess: 'Enabled'
    version: '12.0'
  }
}

resource sqlDatabase 'Microsoft.Sql/servers/databases@2023-08-01' = {
  parent: sqlServer
  name: sqlDatabaseName
  location: location
  sku: {
    name: 'GP_S_Gen5'
    tier: 'GeneralPurpose'
    family: 'Gen5'
    capacity: 2
  }
  tags: mergedTags
  properties: {
    autoPauseDelay: 60
    minCapacity: json('0.5')
  }
}

resource virtualNetworkRule 'Microsoft.Sql/servers/virtualNetworkRules@2023-08-01' = {
  parent: sqlServer
  name: 'aca'
  properties: {
    ignoreMissingVnetServiceEndpoint: false
    virtualNetworkSubnetId: containerAppsSubnet.id
  }
}

resource storageAccount 'Microsoft.Storage/storageAccounts@2024-01-01' = {
  name: storageAccountName
  location: location
  sku: {
    name: 'Standard_LRS'
  }
  kind: 'StorageV2'
  tags: mergedTags
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
            '*'
          ]
          allowedMethods: [
            'GET'
            'HEAD'
            'OPTIONS'
            'PUT'
          ]
          allowedOrigins: [
            'https://${appName}.${containerAppsEnvironment.properties.defaultDomain}'
          ]
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

resource managementPolicy 'Microsoft.Storage/storageAccounts/managementPolicies@2024-01-01' = {
  parent: storageAccount
  name: 'default'
  properties: {
    policy: {
      rules: [
        {
          enabled: true
          name: 'tierScopedArtifacts'
          type: 'Lifecycle'
          definition: {
            actions: {
              baseBlob: {
                tierToArchive: {
                  daysAfterModificationGreaterThan: 180
                }
                tierToCool: {
                  daysAfterModificationGreaterThan: 30
                }
              }
            }
            filters: {
              blobTypes: [
                'blockBlob'
              ]
              prefixMatch: [
                '${blobContainerName}/workspaces/'
              ]
            }
          }
        }
      ]
    }
  }
}

resource sessionPool 'Microsoft.App/sessionPools@2025-10-02-preview' = {
  name: sessionPoolName
  location: location
  tags: mergedTags
  properties: {
    containerType: 'Shell'
    dynamicPoolConfiguration: {
      lifecycleConfiguration: {
        lifecycleType: 'Timed'
        cooldownPeriodInSeconds: 3600
      }
    }
    poolManagementType: 'Dynamic'
    scaleConfiguration: {
      maxConcurrentSessions: 5
    }
    sessionNetworkConfiguration: {
      status: 'EgressEnabled'
    }
  }
}

var appPublicUrl = 'https://${appName}.${containerAppsEnvironment.properties.defaultDomain}'
var appSqlConnectionString = 'Data Source=tcp:${sqlServer.properties.fullyQualifiedDomainName},1433;Initial Catalog=${sqlDatabase.name};User ID=${appManagedIdentity.properties.clientId};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'
var migrationSqlConnectionString = 'Data Source=tcp:${sqlServer.properties.fullyQualifiedDomainName},1433;Initial Catalog=${sqlDatabase.name};User ID=${deploymentManagedIdentity.properties.clientId};Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False'

resource app 'Microsoft.App/containerApps@2024-03-01' = {
  name: appName
  location: location
  identity: {
    type: 'UserAssigned'
    userAssignedIdentities: {
      '${appManagedIdentity.id}': {}
    }
  }
  tags: mergedTags
  properties: {
    managedEnvironmentId: containerAppsEnvironment.id
    configuration: {
      activeRevisionsMode: 'Single'
      ingress: {
        allowInsecure: false
        external: true
        targetPort: 8000
        traffic: [
          {
            latestRevision: true
            weight: 100
          }
        ]
        transport: 'auto'
      }
      secrets: [
        {
          name: sandboxEnvironmentSecretName
          keyVaultUrl: sandboxEnvironmentSecretKeyVaultUrl
          identity: appManagedIdentity.id
        }
      ]
    }
    template: {
      containers: [
        {
          env: [
            {
              name: 'AZURE_CLIENT_ID'
              value: appManagedIdentity.properties.clientId
            }
            {
              name: 'AZURE_SQL_CONNECTIONSTRING'
              value: appSqlConnectionString
            }
            {
              name: 'ADE_SANDBOX_ENVIRONMENT_SECRET'
              secretRef: sandboxEnvironmentSecretName
            }
            {
              name: 'ADE_SESSION_POOL_MANAGEMENT_ENDPOINT'
              value: sessionPool.properties.poolManagementEndpoint
            }
            {
              name: 'ADE_PUBLIC_API_URL'
              value: appPublicUrl
            }
            {
              name: 'ADE_BLOB_ACCOUNT_URL'
              value: storageAccount.properties.primaryEndpoints.blob
            }
            {
              name: 'ADE_BLOB_CONTAINER'
              value: blobContainer.name
            }
          ]
          image: image
          name: appName
          probes: [
            {
              type: 'Startup'
              httpGet: {
                path: '/api/healthz'
                port: 8000
                scheme: 'HTTP'
              }
              initialDelaySeconds: 1
              periodSeconds: 5
              timeoutSeconds: 3
              failureThreshold: 24
              successThreshold: 1
            }
            {
              type: 'Readiness'
              httpGet: {
                path: '/api/readyz'
                port: 8000
                scheme: 'HTTP'
              }
              initialDelaySeconds: 1
              periodSeconds: 5
              timeoutSeconds: 3
              failureThreshold: 3
              successThreshold: 1
            }
            {
              type: 'Liveness'
              httpGet: {
                path: '/api/healthz'
                port: 8000
                scheme: 'HTTP'
              }
              initialDelaySeconds: 10
              periodSeconds: 30
              timeoutSeconds: 3
              failureThreshold: 3
              successThreshold: 1
            }
          ]
          resources: {
            cpu: json(appCpu)
            memory: appMemory
          }
        }
      ]
      scale: {
        maxReplicas: appMaxReplicas
        minReplicas: appMinReplicas
      }
    }
  }
}

resource appSessionPoolExecutorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(sessionPool.id, appManagedIdentity.id, sessionExecutorRoleDefinitionId)
  scope: sessionPool
  properties: {
    principalId: appManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: sessionExecutorRoleDefinitionId
  }
}

resource appStorageBlobDataContributorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(storageAccount.id, appManagedIdentity.id, storageBlobDataContributorRoleDefinitionId)
  scope: storageAccount
  properties: {
    principalId: appManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: storageBlobDataContributorRoleDefinitionId
  }
}

resource migrationJob 'Microsoft.App/jobs@2025-01-01' = {
  name: migrationJobName
  location: location
  identity: {
    type: 'UserAssigned'
    userAssignedIdentities: {
      '${deploymentManagedIdentity.id}': {}
    }
  }
  tags: mergedTags
  properties: {
    configuration: {
      manualTriggerConfig: {
        parallelism: 1
        replicaCompletionCount: 1
      }
      replicaRetryLimit: 0
      replicaTimeout: 1800
      triggerType: 'Manual'
    }
    environmentId: containerAppsEnvironment.id
    template: {
      containers: [
        {
          command: [
            './bin/ade-migrate'
          ]
          env: [
            {
              name: 'AZURE_SQL_CONNECTIONSTRING'
              value: migrationSqlConnectionString
            }
          ]
          image: image
          name: migrationJobName
          resources: {
            cpu: json(jobCpu)
            memory: jobMemory
          }
        }
      ]
    }
  }
}

output appUrl string = appPublicUrl
