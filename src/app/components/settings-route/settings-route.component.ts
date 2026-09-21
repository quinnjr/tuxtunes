import {
  Component,
  OnDestroy,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faXmark } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { TauriService } from '../../services/tauri.service';
import { UiService } from '../../services/ui.service';
import { toErrorMessage } from '../../utils/errors';
import { ModalSheetDirective } from '../../directives/modal-sheet.directive';
import { RovingFocusDirective } from '../../directives/roving-focus.directive';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';
import { SettingsConvertComponent } from '../settings-convert/settings-convert.component';

type SettingsTab = 'playback' | 'convert' | 'sync' | 'maintenance' | 'about';

@Component({
  selector: 'app-settings-route',
  imports: [
    SettingsAudioComponent,
    SettingsConvertComponent,
    ModalSheetDirective,
    FaIconComponent,
    RovingFocusDirective,
  ],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './settings-route.component.html',
})
export class SettingsRouteComponent implements OnDestroy {
  protected readonly sync = inject(SyncService);
  private readonly library = inject(LibraryService);
  private readonly tauri = inject(TauriService);
  protected readonly ui = inject(UiService);

  protected readonly tab = signal<SettingsTab>('playback');
  protected readonly faXmark = faXmark;
  protected readonly tabs: readonly { id: SettingsTab; label: string }[] = [
    { id: 'playback', label: 'Playback' },
    { id: 'convert', label: 'Conversion' },
    { id: 'sync', label: 'Sync Sources' },
    { id: 'maintenance', label: 'Library Maintenance' },
    { id: 'about', label: 'About' },
  ] as const;

  /** Inline status for the verify-library long-running task. */
  protected readonly verifyState = signal<'idle' | 'running' | 'done' | 'error'>('idle');
  /** Set alongside a 'error' verifyState; the message shown near the verify button. */
  protected readonly verifyError = signal<string | null>(null);

  private readonly unlisteners: UnlistenFn[] = [];
  private destroyed = false;
  private verifyTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    // The sheet lives for the whole app session (it is hidden, not
    // unmounted), so the verify-event subscription is established once
    // and torn down on destroy — not per open. Guard the push: the
    // listen promise can resolve after destroy in tests/HMR.
    void this.subscribeVerifyEvents().catch((error: unknown) =>
      console.error('failed to subscribe to fs:verify-failed', error),
    );
    // Sync sources are only needed once the sheet is actually shown.
    effect(() => {
      if (this.ui.settingsOpen()) void this.ui.guard(this.sync.refreshSources());
    });
  }

  ngOnDestroy(): void {
    this.destroyed = true;
    if (this.verifyTimer !== null) {
      clearTimeout(this.verifyTimer);
      this.verifyTimer = null;
    }
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribeVerifyEvents(): Promise<void> {
    // The verify command spawns a background task; if that task fails
    // it reports back via this event since the command boundary itself
    // already returned successfully.
    const off = await this.tauri.listen<{ message: string }>('fs:verify-failed', (payload) => {
      this.verifyState.set('error');
      this.verifyError.set(payload.message);
    });
    if (this.destroyed) {
      off();
      return;
    }
    this.unlisteners.push(off);
  }

  protected setTab(t: SettingsTab): void {
    this.tab.set(t);
  }

  /** Close the sheet; also the Escape target. */
  protected close(): void {
    this.ui.settingsOpen.set(false);
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
      if (this.verifyTimer !== null) clearTimeout(this.verifyTimer);
      this.verifyTimer = setTimeout(() => {
        this.verifyTimer = null;
        // A fs:verify-failed event may have already moved us to 'error'
        // while this timer was pending; don't clobber that back to 'done'.
        if (this.verifyState() === 'error') return;
        void this.ui.guard(this.library.refreshStats());
        this.verifyState.set('done');
      }, 1500);
    } catch (error) {
      this.verifyState.set('error');
      this.verifyError.set(toErrorMessage(error));
    }
  }
}
