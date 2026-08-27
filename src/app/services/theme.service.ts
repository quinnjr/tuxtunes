import { Injectable, computed, effect, signal } from '@angular/core';

export type ColorMode = 'light' | 'dark' | 'system';

const STORAGE_KEY = 'tuxtunes.theme';

/**
 * Owns the color-mode preference (light / dark / system). The resolved
 * theme is written to `<html data-theme>` — styles.css swaps the
 * palette + color-scheme off that attribute — and the preference is
 * persisted to localStorage. "system" tracks the OS
 * `prefers-color-scheme` live; first run defaults to "system".
 */
@Injectable({ providedIn: 'root' })
export class ThemeService {
  readonly mode = signal<ColorMode>(initialMode());

  /** Whether the OS currently prefers dark; kept live via matchMedia. */
  readonly #systemDark = signal(prefersDark());

  /** The actual theme applied to the document. */
  readonly resolved = computed<'light' | 'dark'>(this.#computeResolved.bind(this));

  #computeResolved(): 'light' | 'dark' {
    const mode = this.mode();
    if (mode === 'system') return this.#systemDark() ? 'dark' : 'light';
    return mode;
  }

  constructor() {
    globalThis
      .matchMedia?.('(prefers-color-scheme: dark)')
      .addEventListener('change', this.#onSystemChange);

    effect(() => {
      document.documentElement.dataset['theme'] = this.resolved();
    });
    effect(() => {
      // Storage can be disabled or full (private mode, quota); the
      // preference then just doesn't persist across launches.
      try {
        localStorage.setItem(STORAGE_KEY, this.mode());
      } catch {
        /* best effort */
      }
    });
  }

  set(mode: ColorMode): void {
    this.mode.set(mode);
  }

  readonly #onSystemChange = (e: MediaQueryListEvent): void => {
    this.#systemDark.set(e.matches);
  };
}

function initialMode(): ColorMode {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(STORAGE_KEY);
  } catch {
    /* storage unavailable → default */
  }
  if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
  return 'system';
}

function prefersDark(): boolean {
  return globalThis.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true;
}
