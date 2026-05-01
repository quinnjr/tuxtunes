import { Injectable, signal } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class UiService {
  readonly importWizardOpen = signal(false);
  readonly preferencesOpen = signal(false);
}
