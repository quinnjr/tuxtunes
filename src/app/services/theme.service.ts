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
      localStorage.setItem(STORAGE_KEY, this.mode());
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
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
  return 'system';
}

function prefersDark(): boolean {
  return globalThis.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true;
}
