targetScope = 'resourceGroup'

var location = resourceGroup().location
var mergedTags = {
  project: 'ade'
  environment: 'prod'
}
var deploymentManagedIdentityName = 'id-ade-deploy-prod-canadacentral-002'
var appManagedIdentityName = 'id-ade-app-prod-canadacentral-002'
var githubOidcSubject = 'repo:clac-ca/ade:environment:production'
var keyVaultName = 'kv-ade-prod-cc-002'
var managedIdentityOperatorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'f1a07417-d97a-45cb-824c-7a7467783830')
var contributorRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b24988ac-6180-42a0-ab88-20f7382dd24c')
var keyVaultSecretsUserRoleDefinitionId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '4633458b-17de-408a-b874-0445c86b69e6')

resource deploymentManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: deploymentManagedIdentityName
  location: location
  tags: mergedTags
}

resource appManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: appManagedIdentityName
  location: location
  tags: mergedTags
}

resource deploymentManagedIdentityFederatedCredential 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2024-11-30' = {
  parent: deploymentManagedIdentity
  name: 'github-production'
  properties: {
    audiences: [
      'api://AzureADTokenExchange'
    ]
    issuer: 'https://token.actions.githubusercontent.com'
    subject: githubOidcSubject
  }
}

resource contributorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(resourceGroup().id, deploymentManagedIdentity.id, contributorRoleDefinitionId)
  scope: resourceGroup()
  properties: {
    principalId: deploymentManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: contributorRoleDefinitionId
  }
}

// The deployment identity needs to attach both user-assigned identities during main deployments.
resource managedIdentityOperatorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(deploymentManagedIdentity.id, deploymentManagedIdentity.id, managedIdentityOperatorRoleDefinitionId)
  scope: deploymentManagedIdentity
  properties: {
    principalId: deploymentManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: managedIdentityOperatorRoleDefinitionId
  }
}

resource appManagedIdentityOperatorRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(appManagedIdentity.id, deploymentManagedIdentity.id, managedIdentityOperatorRoleDefinitionId)
  scope: appManagedIdentity
  properties: {
    principalId: deploymentManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: managedIdentityOperatorRoleDefinitionId
  }
}

resource keyVault 'Microsoft.KeyVault/vaults@2023-07-01' = {
  name: keyVaultName
  location: location
  tags: mergedTags
  properties: {
    tenantId: subscription().tenantId
    sku: {
      family: 'A'
      name: 'standard'
    }
    enableRbacAuthorization: true
    enableSoftDelete: true
    enablePurgeProtection: true
  }
}

resource appManagedIdentityKeyVaultSecretsUserRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(keyVault.id, appManagedIdentity.id, keyVaultSecretsUserRoleDefinitionId)
  scope: keyVault
  properties: {
    principalId: appManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: keyVaultSecretsUserRoleDefinitionId
  }
}
