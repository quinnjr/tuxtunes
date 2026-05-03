import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'jsdom',
    include: ['src/**/*.spec.ts'],
    setupFiles: ['src/test-setup.ts'],
    coverage: {
      provider: 'v8',
      include: ['src/app/utils/**/*.ts', 'src/app/services/**/*.ts'],
      exclude: ['src/**/*.spec.ts'],
      reporter: ['text', 'lcov'],
    },
  },
});
