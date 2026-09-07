import {
  Component,
  OnDestroy,
  OnInit,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { LibraryService } from '../../services/library.service';
import { PreferencesService } from '../../services/preferences.service';
import { SyncService } from '../../services/sync.service';
import { TauriService } from '../../services/tauri.service';
import { UiService } from '../../services/ui.service';
import { toErrorMessage } from '../../utils/errors';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';

type SettingsTab = 'playback' | 'sync' | 'maintenance' | 'about';

@Component({
  selector: 'app-settings-route',
  imports: [SettingsAudioComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './settings-route.component.html',
})
export class SettingsRouteComponent implements OnInit, OnDestroy {
  protected readonly sync = inject(SyncService);
  protected readonly prefs = inject(PreferencesService);
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

  /** Set when starting the consolidate pass failed outright. */
  protected readonly consolidateError = signal<string | null>(null);

  /** Inline status for the verify-library long-running task. */
  protected readonly verifyState = signal<'idle' | 'running' | 'done' | 'error'>('idle');
  /** Set alongside a 'error' verifyState; the message shown near the verify button. */
  protected readonly verifyError = signal<string | null>(null);

  private readonly unlisteners: UnlistenFn[] = [];

  constructor() {
    void this.subscribeFsEvents().catch((error: unknown) =>
      console.error('failed to subscribe to fs events', error),
    );
  }

  ngOnInit(): void {
    void this.sync.refreshSources();
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribeFsEvents(): Promise<void> {
    this.unlisteners.push(
      // The moved/copied paths are only in the DB, so reload once the
      // pass reports in.
      await this.tauri.listen<unknown>('fs:consolidate-complete', () => {
        void this.library.refreshTracks();
      }),
      // The verify command spawns a background task; if that task fails
      // it reports back via this event since the command boundary itself
      // already returned successfully.
      await this.tauri.listen<{ message: string }>('fs:verify-failed', (payload) => {
        this.verifyState.set('error');
        this.verifyError.set(payload.message);
      }),
    );
  }

  /**
   * Reorganize every track into the library folder. The pass runs on
   * the backend's ingest queue and reports through PreferencesService;
   * refresh the list once it finishes so the moved paths show up.
   */
  protected async consolidate(): Promise<void> {
    this.consolidateError.set(null);
    try {
      await this.prefs.consolidateLibrary();
    } catch (error) {
      this.consolidateError.set(toErrorMessage(error));
    }
  }

  /** `current of total` for the progress line, or null when idle. */
  protected consolidateStatus(): string | null {
    const p = this.prefs.consolidateProgress();
    if (!p) return null;
    return p.total > 0 ? `Reorganizing ${p.current} of ${p.total}…` : 'Starting…';
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
    this.verifyError.set(null);
    try {
      await this.tauri.invoke<void>('verify_library');
      // Verify is fire-and-forget at the command boundary; we don't have
      // a typed completion event yet, so reflect that with a 'done'
      // marker and a stats refresh after a short settle.
      setTimeout(() => {
        // A fs:verify-failed event may have already moved us to 'error'
        // while this timer was pending; don't clobber that back to 'done'.
        if (this.verifyState() === 'error') return;
        void this.library.refreshStats();
        this.verifyState.set('done');
      }, 1500);
    } catch (error) {
      this.verifyState.set('error');
      this.verifyError.set(toErrorMessage(error));
    }
  }
}
