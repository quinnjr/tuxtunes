import { Component, effect, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import { PreferencesService } from '../../services/preferences.service';
import { UiService } from '../../services/ui.service';

@Component({
  selector: 'app-preferences-panel',
  imports: [FormsModule],
  templateUrl: './preferences-panel.component.html',
})
export class PreferencesPanelComponent {
  protected readonly prefs = inject(PreferencesService);
  protected readonly open = inject(UiService).preferencesOpen;

  protected readonly draftRoot = signal('');
  protected readonly draftScheme = signal('');
  protected readonly draftKeep = signal(true);

  constructor() {
    effect(() => {
      if (this.open()) {
        void this.prefs.refresh().then(() => {
          this.draftRoot.set(this.prefs.libraryRoot());
          this.draftScheme.set(this.prefs.organizeScheme());
          this.draftKeep.set(this.prefs.keepOrganized());
        });
      }
    });
  }

  protected async pickRoot(): Promise<void> {
    const picked = await dialogOpen({ directory: true, multiple: false });
    if (typeof picked === 'string') this.draftRoot.set(picked);
  }

  protected async save(): Promise<void> {
    await Promise.all([
      this.prefs.setLibraryRoot(this.draftRoot()),
      this.prefs.setOrganizeScheme(this.draftScheme()),
      this.prefs.setKeepOrganized(this.draftKeep()),
    ]);
    this.hide();
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
