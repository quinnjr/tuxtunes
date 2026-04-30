import { Injectable } from '@angular/core';
import { invoke as coreInvoke } from '@tauri-apps/api/core';
import { listen as coreListen, type UnlistenFn } from '@tauri-apps/api/event';

@Injectable({ providedIn: 'root' })
export class TauriService {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    return coreInvoke<T>(command, args);
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
    return coreListen<T>(event, (e) => handler(e.payload));
  }
}
