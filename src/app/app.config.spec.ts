import { describe, expect, it } from 'vitest';
import { appConfig } from './app.config';
import { IconRegistry } from './icons/icon-registry';

describe('appConfig', () => {
  it('registers the IconRegistry provider', () => {
    expect(appConfig.providers.length).toBeGreaterThan(0);
    expect(appConfig.providers).toContain(IconRegistry);
  });
});
