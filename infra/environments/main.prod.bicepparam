using '../main.bicep'

param prefix = 'ade-prod'
param tags = {
  environment: 'prod'
}
param webImage = readEnvironmentVariable('ADE_WEB_IMAGE')
param apiImage = readEnvironmentVariable('ADE_API_IMAGE')
