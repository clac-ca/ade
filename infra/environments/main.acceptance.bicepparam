using '../main.bicep'

param prefix = 'ade-acceptance'
param tags = {
  environment: 'acceptance'
}
param image = readEnvironmentVariable('ADE_IMAGE')
param registryServer = readEnvironmentVariable('ADE_REGISTRY_SERVER', '')
param registryUsername = readEnvironmentVariable('ADE_REGISTRY_USERNAME', '')
param registryPassword = readEnvironmentVariable('ADE_REGISTRY_PASSWORD', '')
