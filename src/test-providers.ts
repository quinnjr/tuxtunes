import { provideZonelessChangeDetection } from '@angular/core';

/**
 * Test-environment providers for Angular's unit-test builder. Mirrors
 * the production app config (zoneless change detection) so component
 * fixtures behave the same way they do at runtime.
 */
export default [provideZonelessChangeDetection()];
