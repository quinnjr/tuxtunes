import { Component, OnInit, inject, signal } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { TauriService } from '../../services/tauri.service';
import { UiService } from '../../services/ui.service';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';

type SettingsTab = 'playback' | 'sync' | 'maintenance' | 'about';

@Component({
  selector: 'app-settings-route',
  imports: [SettingsAudioComponent],
  templateUrl: './settings-route.component.html',
})
export class SettingsRouteComponent implements OnInit {
  protected readonly sync = inject(SyncService);
  private readonly library = inject(LibraryService);
  private readonly tauri = inject(TauriService);
  private readonly ui = inject(UiService);

  protected readonly tab = signal<SettingsTab>('playback');
  protected readonly tabs: readonly { id: SettingsTab; label: string }[] = [
    { id: 'playback', label: 'Playback' },
    { id: 'sync', label: 'Sync Sources' },
    { id: 'maintenance', label: 'Library Maintenance' },
    { id: 'about', label: 'About' },
  ] as const;

  /** Inline status for the verify-library long-running task. */
  protected readonly verifyState = signal<'idle' | 'running' | 'done'>('idle');

  ngOnInit(): void {
    void this.sync.refreshSources();
  }

  protected setTab(t: SettingsTab): void {
    this.tab.set(t);
  }

  protected formatLastSync(iso: string | null): string {
    if (!iso) return 'Never';
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
  }

  protected async runSync(sourceId: number): Promise<void> {
    await this.sync.runNow(sourceId);
  }

  protected openImportWizard(): void {
    this.ui.importWizardOpen.set(true);
  }

  protected openLibraryPrefs(): void {
    this.ui.preferencesOpen.set(true);
  }

  /**
   * Kick off the verify walk and refresh stats once it's done. The
   * backend command returns immediately (it spawns a background task);
   * we listen for completion via library stats settling.
   */
  protected async verify(): Promise<void> {
    this.verifyState.set('running');
    try {
      await this.tauri.invoke<void>('verify_library');
      // Verify is fire-and-forget at the command boundary; we don't have
      // a typed completion event yet, so reflect that with a 'done'
      // marker and a stats refresh after a short settle.
      setTimeout(() => {
        void this.library.refreshStats();
        this.verifyState.set('done');
      }, 1500);
    } catch {
      this.verifyState.set('idle');
    }
  }
}
