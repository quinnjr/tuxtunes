import { Injectable, inject, signal } from '@angular/core';
import { TauriService } from './tauri.service';

@Injectable({ providedIn: 'root' })
export class PreferencesService {
  private readonly tauri = inject(TauriService);

  readonly libraryRoot = signal<string>('');
  readonly organizeScheme = signal<string>('');
  readonly keepOrganized = signal<boolean>(true);

  async refresh(): Promise<void> {
    const [root, scheme, keep] = await Promise.all([
      this.tauri.invoke<string>('get_library_root'),
      this.tauri.invoke<string>('get_organize_scheme'),
      this.tauri.invoke<boolean>('get_keep_organized'),
    ]);
    this.libraryRoot.set(root);
    this.organizeScheme.set(scheme);
    this.keepOrganized.set(keep);
  }

  async setLibraryRoot(path: string): Promise<void> {
    await this.tauri.invoke<void>('set_library_root', { path });
    this.libraryRoot.set(path);
  }

  async setOrganizeScheme(scheme: string): Promise<void> {
    await this.tauri.invoke<void>('set_organize_scheme', { scheme });
    this.organizeScheme.set(scheme);
  }

  async setKeepOrganized(keep: boolean): Promise<void> {
    await this.tauri.invoke<void>('set_keep_organized', { keep });
    this.keepOrganized.set(keep);
  }
}
