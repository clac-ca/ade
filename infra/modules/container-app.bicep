param name string
param location string = resourceGroup().location
param managedEnvironmentId string
param image string
param containerPort int
param externalIngress bool = false
param allowInsecure bool = false
@allowed([
  'auto'
  'http'
  'http2'
])
param transport string = 'auto'
param env array = []
param cpu string = '0.25'
param memory string = '0.5Gi'
param minReplicas int = 1
param maxReplicas int = 1
param tags object = {}
@description('Container registry server used to pull the image, for example `ghcr.io`.')
param registryServer string = ''
@description('Username for the configured container registry.')
param registryUsername string = ''
@description('Password or token for the configured container registry.')
@secure()
param registryPassword string = ''

var usesRegistry = registryServer != ''
var usesRegistryCredentials = registryServer != '' && registryUsername != '' && registryPassword != ''

resource containerApp 'Microsoft.App/containerApps@2024-03-01' = {
  name: name
  location: location
  tags: tags
  properties: {
    managedEnvironmentId: managedEnvironmentId
    configuration: union({
      activeRevisionsMode: 'Single'
      ingress: {
        allowInsecure: allowInsecure
        external: externalIngress
        targetPort: containerPort
        traffic: [
          {
            latestRevision: true
            weight: 100
          }
        ]
        transport: transport
      }
    }, usesRegistry ? {
      registries: [
        union({
          server: registryServer
        }, usesRegistryCredentials ? {
          passwordSecretRef: 'registry-password'
          username: registryUsername
        } : {})
      ]
    } : {}, usesRegistryCredentials ? {
      secrets: [
        {
          name: 'registry-password'
          value: registryPassword
        }
      ]
    } : {})
    template: {
      containers: [
        {
          env: env
          image: image
          name: name
          resources: {
            cpu: json(cpu)
            memory: memory
          }
        }
      ]
      scale: {
        maxReplicas: maxReplicas
        minReplicas: minReplicas
      }
    }
  }
}

output id string = containerApp.id
output fqdn string = containerApp.properties.configuration.ingress.fqdn
output url string = '${externalIngress ? 'https' : 'http'}://${containerApp.properties.configuration.ingress.fqdn}'
