// Global test setup, run before every spec file (wired via the
// `setupFiles` option of @angular/build:unit-test in angular.json).
//
// The no-browser Vitest mode provides DOM globals but not Web Storage,
// and Node's own experimental `localStorage` global is `undefined`
// unless the process is started with `--localstorage-file`. Install a
// spec-compliant in-memory implementation so services and specs can use
// `localStorage`/`sessionStorage` unconditionally.

class MemoryStorage implements Storage {
  #store = new Map<string, string>();

  get length(): number {
    return this.#store.size;
  }

  clear(): void {
    this.#store.clear();
  }

  getItem(key: string): string | null {
    return this.#store.get(key) ?? null;
  }

  key(index: number): string | null {
    return [...this.#store.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.#store.delete(key);
  }

  setItem(key: string, value: string): void {
    this.#store.set(key, String(value));
  }
}

for (const name of ['localStorage', 'sessionStorage'] as const) {
  if (globalThis[name] === undefined) {
    Object.defineProperty(globalThis, name, {
      value: new MemoryStorage(),
      configurable: true,
      writable: true,
    });
  }
}
