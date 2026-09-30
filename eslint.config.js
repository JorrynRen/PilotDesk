import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      // `_` / `__` 是刻意的丢弃标记，最常见的是"省略某个键"的写法：
      //   `const { [id]: _, ...rest } = map`
      // 默认配置不认这个约定，会把 `_` 报成"未使用变量"。按 TS-ESLint 官方推荐显式声明忽略模式：
      // 这几项只影响**下划线开头**的标识符，不会掩盖真名字的未使用变量；
      // `ignoreRestSiblings` 让"取剩余属性时被丢弃的那个兄弟键"免报（即上面那种省略写法）。
      '@typescript-eslint/no-unused-vars': ['error', {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^_',
        caughtErrorsIgnorePattern: '^_',
        destructuredArrayIgnorePattern: '^_',
        ignoreRestSiblings: true,
      }],
    },
  },
])
