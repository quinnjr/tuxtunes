import { ApplicationConfig, provideBrowserGlobalErrorListeners } from '@angular/core';
import { FaConfig, FaIconLibrary } from '@fortawesome/angular-fontawesome';
import { fas } from '@fortawesome/free-solid-svg-icons';
import { far } from '@fortawesome/free-regular-svg-icons';

function provideFontAwesome() {
  return {
    provide: FaConfig,
    useFactory: (library: FaIconLibrary) => {
      library.addIconPacks(fas, far);
      return new FaConfig();
    },
    deps: [FaIconLibrary],
  };
}

export const appConfig: ApplicationConfig = {
  providers: [provideBrowserGlobalErrorListeners(), provideFontAwesome()],
};
