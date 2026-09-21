import { Injectable, computed, signal } from '@angular/core';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import { toErrorMessage } from '../utils/errors';

export type LibraryView = 'tracks' | 'albums' | 'artists' | 'genres' | 'queue' | 'device';

/**
 * How an open playlist is presented: `albums` is the per-album picker
 * (artwork cards that drop down into their tracks), `songs` the flat
 * list in playlist order. The library's tracks/albums/artists split
 * only applies to "all songs".
 */
export type PlaylistView = 'albums' | 'songs';

export interface NamePromptRequest {
  title: string;
  initial: string;
  onSubmit: (name: string) => void | Promise<void>;
}

export interface ConfirmRequest {
  title: string;
  message: string;
  /** Label for the confirming button, e.g. "Delete Folder". */
  confirmLabel: string;
  destructive?: boolean;
  onConfirm: () => void | Promise<void>;
}

/**
 * Album-card orderings. `playlist` is the order the playlist first
 * reaches each album; the rest sort by an album-level value derived
 * from its tracks (see `PlaylistAlbum`).
 */
export type PlaylistAlbumSortKey = 'playlist' | 'name' | 'artist' | 'year' | 'rating' | 'dateAdded';

export interface PlaylistAlbumSort {
  key: PlaylistAlbumSortKey;
  descending: boolean;
}

export const DEFAULT_PLAYLIST_ALBUM_SORT: PlaylistAlbumSort = {
  key: 'playlist',
  descending: false,
};

export const PLAYLIST_ALBUM_SORT_KEYS: readonly PlaylistAlbumSortKey[] = [
  'playlist',
  'name',
  'artist',
  'year',
  'rating',
  'dateAdded',
] as const;

/** Newest / highest first is the natural reading for these. */
export function defaultDescending(key: PlaylistAlbumSortKey): boolean {
  return key === 'year' || key === 'rating' || key === 'dateAdded';
}

@Injectable({ providedIn: 'root' })
export class UiService {
  readonly importWizardOpen = signal(false);
  readonly preferencesOpen = signal(false);

  /**
   * Full-screen Settings sheet (playback / conversion / sync /
   * maintenance / about). Opened from the menu bar's Settings menu.
   */
  readonly settingsOpen = signal(false);

  /** Top-level view selection. Drives main-content's active component. */
  readonly libraryView = signal<LibraryView>('tracks');

  /** Presentation of the active playlist; sticky across playlists. */
  readonly playlistView = signal<PlaylistView>('albums');

  /** Ordering of the cards in a playlist's album view; sticky across playlists. */
  readonly playlistAlbumSort = signal<PlaylistAlbumSort>({ ...DEFAULT_PLAYLIST_ALBUM_SORT });

  /** Whether the column browser strip is shown above the active view. */
  readonly columnBrowserOpen = signal(false);

  /**
   * Which device the `'device'` view is showing. Kept separate from
   * `libraryView` so returning to a library view and back does not
   * lose the user's place.
   */
  readonly activeDeviceId = signal<number | null>(null);

  /** Sidebar folder ids the user has expanded. Folders start collapsed. */
  readonly expandedFolders = signal<Set<number>>(new Set<number>());

  /** Whether the Now Playing slide-out is visible. */
  readonly nowPlayingOpen = signal(false);

  /**
   * Smart-playlist editor: null = closed; `{ playlistId: null }` = new
   * playlist; a number = editing that smart playlist's rule.
   */
  readonly smartEditor = signal<{ playlistId: number | null } | null>(null);

  /**
   * In-app replacement for `window.prompt`: null = closed; otherwise
   * the modal shows `title` with `initial` in the input and calls
   * `onSubmit` with the trimmed non-empty name.
   */
  readonly namePrompt = signal<NamePromptRequest | null>(null);

  /**
   * In-app confirmation dialog for actions that destroy more than the
   * thing that was clicked (deleting a folder full of playlists).
   */
  readonly confirm = signal<ConfirmRequest | null>(null);

  /** Track-info (Get Info…) editor: null = closed. */
  readonly trackInfo = signal<{ trackId: number } | null>(null);

  /**
   * Whether any modal owns the screen. Keyboard shortcuts bound on
   * document check this so a list-level key (Delete) does not fire
   * behind an open dialog. Drives `[inert]` on the app shell.
   */
  readonly anyModalOpen = computed(this.#computeAnyModalOpen.bind(this));

  #computeAnyModalOpen(): boolean {
    return (
      this.importWizardOpen() ||
      this.preferencesOpen() ||
      this.settingsOpen() ||
      this.smartEditor() !== null ||
      this.namePrompt() !== null ||
      this.confirm() !== null ||
      this.trackInfo() !== null
    );
  }

  /**
   * Whether a menu-bar dropdown is open. Written by `MenuBarComponent`.
   * A dropdown must not make the shell inert, but document-level list
   * shortcuts (Delete) must not fire behind it either.
   */
  readonly menubarOpen = signal(false);

  /**
   * Anything that should suppress document-level single-key shortcuts:
   * modals, an open menu-bar dropdown, or the column picker. The context
   * menu is not a modal but is checked separately by its consumer.
   */
  readonly shortcutsBlocked = computed(this.#computeShortcutsBlocked.bind(this));

  #computeShortcutsBlocked(): boolean {
    return this.anyModalOpen() || this.menubarOpen() || this.columnPickerOpen();
  }

  /**
   * The track list's column-picker popover. Owned by the track list, but
   * surfaced here so the shared shortcut guard can see it.
   */
  readonly columnPickerOpen = signal(false);

  /**
   * Most recent user-facing failure (a backend command rejected, a
   * file could not be played, …). Shown by the status bar and cleared
   * automatically after a few seconds or on the next `clearError()`.
   */
  readonly lastError = signal<string | null>(null);
  private errorTimer: ReturnType<typeof setTimeout> | null = null;
  static readonly ERROR_VISIBLE_MS = 6000;

  reportError(error: unknown): void {
    this.setError(toErrorMessage(error));
  }

  clearError(): void {
    this.setError(null);
  }

  /**
   * Await `promise`, reporting a rejection instead of letting it
   * escape as an unhandled rejection. Resolves to the value, or null
   * when it failed. The idiom for fire-and-forget UI work:
   * `void this.ui.guard(this.library.refreshTracks())`.
   */
  async guard<T>(promise: Promise<T>): Promise<T | null> {
    try {
      return await promise;
    } catch (error) {
      this.reportError(error);
      return null;
    }
  }

  /** Native folder picker; null when cancelled or when the dialog failed. */
  async pickDirectory(): Promise<string | null> {
    const picked = await this.guard(dialogOpen({ directory: true, multiple: false }));
    return typeof picked === 'string' ? picked : null;
  }

  private setError(message: string | null): void {
    if (this.errorTimer !== null) {
      clearTimeout(this.errorTimer);
      this.errorTimer = null;
    }
    this.lastError.set(message);
    if (message !== null) {
      this.errorTimer = setTimeout(() => {
        this.lastError.set(null);
        this.errorTimer = null;
      }, UiService.ERROR_VISIBLE_MS);
    }
  }
}
