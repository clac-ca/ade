using '../main.bicep'

param prefix = 'ade-acceptance'
param tags = {
  environment: 'acceptance'
}
param webImage = readEnvironmentVariable('ADE_WEB_IMAGE')
param apiImage = readEnvironmentVariable('ADE_API_IMAGE')
param registryServer = readEnvironmentVariable('ADE_REGISTRY_SERVER', '')
param registryUsername = readEnvironmentVariable('ADE_REGISTRY_USERNAME', '')
param registryPassword = readEnvironmentVariable('ADE_REGISTRY_PASSWORD', '')
