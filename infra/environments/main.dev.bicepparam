using '../main.bicep'

param prefix = 'ade-dev'
param tags = {
  environment: 'dev'
}
param webImage = readEnvironmentVariable('ADE_WEB_IMAGE')
param apiImage = readEnvironmentVariable('ADE_API_IMAGE')
