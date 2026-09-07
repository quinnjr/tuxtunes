import { Component, effect, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import { LibraryService } from '../../services/library.service';
import { PreferencesService } from '../../services/preferences.service';
import { ColorMode, ThemeService } from '../../services/theme.service';
import { UiService } from '../../services/ui.service';
import { formatByteSize } from '../../utils/format';

@Component({
  selector: 'app-preferences-panel',
  imports: [FormsModule],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './preferences-panel.component.html',
})
export class PreferencesPanelComponent {
  protected readonly prefs = inject(PreferencesService);
  protected readonly theme = inject(ThemeService);
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);
  protected readonly open = this.ui.preferencesOpen;

  /** Color-mode choices, in display order, for the segmented selector. */
  protected readonly colorModes: readonly ColorMode[] = ['light', 'dark', 'system'] as const;

  protected readonly draftRoot = signal('');
  protected readonly draftScheme = signal('');
  protected readonly draftKeep = signal(true);

  constructor() {
    effect(() => {
      if (this.open()) {
        void this.ui.guard(this.prefs.refresh()).then((ok) => {
          if (ok === null) return;
          this.draftRoot.set(this.prefs.libraryRoot());
          this.draftScheme.set(this.prefs.organizeScheme());
          this.draftKeep.set(this.prefs.keepOrganized());
          void this.ui.guard(this.prefs.refreshReclaimEstimate());
        });
      }
    });
  }

  protected async pickRoot(): Promise<void> {
    const picked = await this.ui.guard(dialogOpen({ directory: true, multiple: false }));
    if (typeof picked === 'string') this.draftRoot.set(picked);
  }

  /** Persist the draft; on failure report it and keep the dialog open. */
  protected async save(): Promise<void> {
    const ok = await this.ui.guard(
      Promise.all([
        this.prefs.setLibraryRoot(this.draftRoot()),
        this.prefs.setOrganizeScheme(this.draftScheme()),
        this.prefs.setKeepOrganized(this.draftKeep()),
      ]),
    );
    if (ok !== null) this.hide();
  }

  /** Result line for a finished reorganize pass. */
  protected reorganizeSummary(): string | null {
    const r = this.prefs.consolidateResult();
    if (r === null) return null;
    const parts = [`${r.moved} moved`, `${r.copied} copied`, `${r.in_place} already in place`];
    // Rows whose file is not on disk are not failures — an imported
    // library is full of them.
    if (r.missing > 0) parts.push(`${r.missing} file${r.missing === 1 ? '' : 's'} not found`);
    if (r.failed > 0) parts.push(`${r.failed} failed`);
    return parts.join(', ');
  }

  protected hide(): void {
    this.open.set(false);
  }

  /**
   * Bring every track to the path the organize scheme asks for. Saves
   * the draft first, so the pass uses the root and scheme on screen
   * rather than whatever was stored when the dialog opened.
   */
  protected async reorganize(): Promise<void> {
    const saved = await this.ui.guard(
      Promise.all([
        this.prefs.setLibraryRoot(this.draftRoot()),
        this.prefs.setOrganizeScheme(this.draftScheme()),
        this.prefs.setKeepOrganized(this.draftKeep()),
      ]),
    );
    if (saved === null) return;
    await this.ui.guard(this.prefs.consolidateLibrary());
  }

  /**
   * Trash the originals left behind by copying into the library. The
   * confirmation names the number of files and the space, because it
   * is the one action here that removes something.
   */
  protected reclaim(): void {
    const est = this.prefs.reclaimEstimate();
    if (est === null || est.files === 0) return;
    this.ui.confirm.set({
      title: 'Reclaim Space',
      message:
        `Move ${est.files.toLocaleString()} original file${est.files === 1 ? '' : 's'} ` +
        `(${formatByteSize(est.bytes)}) to the trash? Each one is checked against its copy in ` +
        `the library folder first, and anything that does not match byte for byte is left alone.`,
      confirmLabel: 'Move to Trash',
      destructive: true,
      onConfirm: async () => {
        await this.ui.guard(this.prefs.reclaimOriginals());
      },
    });
  }

  /** `current of total` while the reclaim runs, else null. */
  protected reclaimStatus(): string | null {
    const p = this.prefs.reclaimProgress();
    if (!p) return null;
    return p.total > 0 ? `Reclaiming ${p.current} of ${p.total}…` : 'Starting…';
  }

  /** What a reclaim would free, phrased for the button's caption. */
  protected reclaimOffer(): string | null {
    const est = this.prefs.reclaimEstimate();
    if (est === null) return null;
    if (est.files === 0) return 'No duplicated originals to reclaim.';
    return `${est.files.toLocaleString()} original file${est.files === 1 ? '' : 's'} (${formatByteSize(est.bytes)}) can be reclaimed.`;
  }

  /** Result line for a finished reclaim. */
  protected reclaimSummary(): string | null {
    const r = this.prefs.reclaimResult();
    if (r === null) return null;
    const parts = [`${r.reclaimed.toLocaleString()} trashed (${formatByteSize(r.bytes_freed)})`];
    if (r.skipped > 0) parts.push(`${r.skipped.toLocaleString()} left alone`);
    if (r.failed > 0) parts.push(`${r.failed.toLocaleString()} failed`);
    return parts.join(', ');
  }

  /** `current of total` while the pass runs, else null. */
  protected reorganizeStatus(): string | null {
    const p = this.prefs.consolidateProgress();
    if (!p) return null;
    return p.total > 0 ? `Reorganizing ${p.current} of ${p.total}…` : 'Starting…';
  }

  protected toggleKeep(): void {
    this.draftKeep.update((v) => !v);
  }

  /** Live preview of the organize-scheme template against a sample track. */
  protected preview(): string {
    const scheme =
      this.draftScheme() || '{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}';
    const sample: Record<string, string> = {
      '{album_artist}': 'The Beatles',
      '{artist}': 'The Beatles',
      '{album}': 'Abbey Road',
      '{title}': 'Something',
      '{genre}': 'Rock',
      '{year}': '1969',
      '{track}': '3',
      '{track:02}': '03',
      '{disc}': '1',
      '{disc:02}': '01',
      '{ext}': 'flac',
    };
    let out = scheme;
    for (const [token, val] of Object.entries(sample)) {
      out = out.replaceAll(token, val);
    }
    return out;
  }
}
