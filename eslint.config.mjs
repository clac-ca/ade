import js from '@eslint/js'
import { fileURLToPath } from 'node:url'
import globals from 'globals'
import tseslint from 'typescript-eslint'

const rootDir = fileURLToPath(new URL('.', import.meta.url))

export default tseslint.config(
  {
    ignores: ['**/dist/**', '**/node_modules/**', 'apps/ade-api/.package/**', '.buildx-cache/**']
  },
  {
    files: ['**/*.{js,mjs,cjs}'],
    extends: [js.configs.recommended],
    languageOptions: {
      globals: {
        ...globals.node
      }
    }
  },
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      ...tseslint.configs.recommendedTypeChecked,
      ...tseslint.configs.strictTypeChecked
    ],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: rootDir
      },
      globals: {
        ...globals.node
      }
    },
    rules: {
      '@typescript-eslint/no-unsafe-argument': 'error',
      '@typescript-eslint/no-unsafe-assignment': 'error',
      '@typescript-eslint/no-unsafe-call': 'error',
      '@typescript-eslint/no-unsafe-member-access': 'error',
      '@typescript-eslint/no-unsafe-return': 'error'
    }
  },
  {
    files: ['apps/ade-web/src/**/*.{ts,tsx}', 'apps/ade-web/vite.config.ts'],
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node
      }
    }
  },
  {
    files: ['apps/ade-api/test/**/*.ts', 'scripts/test/**/*.ts'],
    rules: {
      '@typescript-eslint/no-floating-promises': 'off',
      '@typescript-eslint/no-unnecessary-type-parameters': 'off',
      '@typescript-eslint/require-await': 'off',
      '@typescript-eslint/restrict-template-expressions': 'off'
    }
  }
)
