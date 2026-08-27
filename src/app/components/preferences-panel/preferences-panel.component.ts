import { Component, effect, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import { PreferencesService } from '../../services/preferences.service';
import { ColorMode, ThemeService } from '../../services/theme.service';
import { UiService } from '../../services/ui.service';

@Component({
  selector: 'app-preferences-panel',
  imports: [FormsModule],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './preferences-panel.component.html',
})
export class PreferencesPanelComponent {
  protected readonly prefs = inject(PreferencesService);
  protected readonly theme = inject(ThemeService);
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

  protected hide(): void {
    this.open.set(false);
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
