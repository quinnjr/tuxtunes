import { describe, expect, it, vi } from 'vitest';

// Module-level mocks must be hoisted via vi.mock; the factory can't
// reference outer scope, so we expose the spies through the mocked
// module and re-import them inside each test.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

import { invoke as mockInvoke } from '@tauri-apps/api/core';
import { listen as mockListen } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';

describe('TauriService', () => {
  it('invoke() forwards command + args to @tauri-apps/api/core', async () => {
    (mockInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(42);
    const svc = new TauriService();
    const out = await svc.invoke<number>('answer', { question: 'life' });
    expect(out).toBe(42);
    expect(mockInvoke).toHaveBeenCalledWith('answer', { question: 'life' });
  });

  it('listen() unwraps the event envelope before calling the handler', async () => {
    const unlisten = vi.fn();
    (mockListen as ReturnType<typeof vi.fn>).mockImplementationOnce(
      (_event: string, cb: (e: { payload: unknown }) => void) => {
        // Simulate the runtime delivering one event before returning.
        cb({ payload: { value: 7 } });
        return Promise.resolve(unlisten);
      },
    );
    const svc = new TauriService();
    const handler = vi.fn();
    const off = await svc.listen<{ value: number }>('chan', handler);
    expect(handler).toHaveBeenCalledWith({ value: 7 });
    expect(off).toBe(unlisten);
  });
});
