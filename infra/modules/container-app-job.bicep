param name string
param managedEnvironmentId string
param image string
param deploymentManagedIdentityResourceId string
param env array = []
param command array = []
param cpu string = '0.25'
param memory string = '0.5Gi'
param replicaTimeout int = 1800
param tags object = {}
param location string = resourceGroup().location

resource job 'Microsoft.App/jobs@2025-01-01' = {
  name: name
  location: location
  identity: {
    type: 'UserAssigned'
    userAssignedIdentities: {
      '${deploymentManagedIdentityResourceId}': {}
    }
  }
  tags: tags
  properties: {
    configuration: {
      manualTriggerConfig: {
        parallelism: 1
        replicaCompletionCount: 1
      }
      replicaRetryLimit: 0
      replicaTimeout: replicaTimeout
      triggerType: 'Manual'
    }
    environmentId: managedEnvironmentId
    template: {
      containers: [
        {
          command: command
          env: env
          image: image
          name: name
          resources: {
            cpu: json(cpu)
            memory: memory
          }
        }
      ]
    }
  }
}

output id string = job.id
