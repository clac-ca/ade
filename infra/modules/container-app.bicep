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
param secrets array = []
param probes array = []
param cpu string = '0.25'
param memory string = '0.5Gi'
param minReplicas int = 1
param maxReplicas int = 1
param tags object = {}

resource containerApp 'Microsoft.App/containerApps@2024-03-01' = {
  name: name
  location: location
  identity: {
    type: 'SystemAssigned'
  }
  tags: tags
  properties: {
    managedEnvironmentId: managedEnvironmentId
    configuration: {
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
      secrets: secrets
    }
    template: {
      containers: [
        {
          env: env
          image: image
          name: name
          probes: probes
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
output principalId string = containerApp.identity.principalId
output url string = '${externalIngress ? 'https' : 'http'}://${containerApp.properties.configuration.ingress.fqdn}'
