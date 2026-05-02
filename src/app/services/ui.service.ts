import { Injectable, signal } from '@angular/core';

export type LibraryView = 'tracks' | 'albums' | 'artists' | 'genres' | 'settings';

@Injectable({ providedIn: 'root' })
export class UiService {
  readonly importWizardOpen = signal(false);
  readonly preferencesOpen = signal(false);

  /** Top-level view selection. Drives main-content's active component. */
  readonly libraryView = signal<LibraryView>('tracks');

  /** Whether the column browser strip is shown above the active view. */
  readonly columnBrowserOpen = signal(false);

  /** Whether the Now Playing slide-out is visible. */
  readonly nowPlayingOpen = signal(false);
}
