import { ApplicationConfig, provideBrowserGlobalErrorListeners } from '@angular/core';
import { IconRegistry } from './icons/icon-registry';

export const appConfig: ApplicationConfig = {
  providers: [provideBrowserGlobalErrorListeners(), IconRegistry],
};
