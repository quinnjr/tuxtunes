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

// The Tauri runtime global the real `@tauri-apps/api/core` reads.
// Specs also `vi.mock` that module, but a filtered run
// (`ng test --include <one file>`) serves some import graphs the
// unmocked copy, so artwork tests fail solo while passing in the full
// suite. This stub makes the real `convertFileSrc` behave identically
// in every mode. Keep the return shape in sync with the `vi.mock`
// factories in the component specs (`asset://` prefix); `invoke` is
// deliberately unstubbed — every test reaches it through the
// `TauriService` stub instead, and an unmocked call should stay loud.
{
  const globals = globalThis as unknown as Record<string, unknown>;
  if (globals['__TAURI_INTERNALS__'] === undefined) {
    Object.defineProperty(globalThis, '__TAURI_INTERNALS__', {
      value: {
        convertFileSrc: (filePath: string) => `asset://${filePath}`,
      },
      configurable: true,
      writable: true,
    });
  }
}
