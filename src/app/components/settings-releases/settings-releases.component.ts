import { Component, OnInit, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import {
  DEFAULT_RELEASE_PREFS,
  ReleasePrefs,
  ReleasesService,
} from '../../services/releases.service';
import { UiService } from '../../services/ui.service';
import { Choice, ChoiceSelectComponent } from '../choice-select/choice-select.component';

@Component({
  selector: 'app-settings-releases',
  imports: [ChoiceSelectComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './settings-releases.component.html',
})
export class SettingsReleasesComponent implements OnInit {
  private readonly releases = inject(ReleasesService);
  private readonly ui = inject(UiService);

  protected readonly draft = signal<ReleasePrefs>(DEFAULT_RELEASE_PREFS);
  /** The form stays disabled until the stored prefs are in; see SettingsConvertComponent. */
  protected readonly loaded = signal(false);

  protected readonly windows: readonly Choice<number>[] = [
    { value: 30, label: '30 days' },
    { value: 60, label: '60 days' },
    { value: 90, label: '90 days' },
    { value: 180, label: '6 months' },
    { value: 365, label: 'A year' },
  ] as const;

  ngOnInit(): void {
    void this.reload();
  }

  protected async reload(): Promise<void> {
    const prefs = await this.ui.guard(this.releases.getPrefs());
    if (prefs === null) return;
    this.draft.set(prefs);
    this.loaded.set(true);
  }

  /**
   * Persist a change and re-seed from what the backend stored; a failed
   * save rolls back. The backend starts a check itself when the change
   * has something to fetch (a window wider than the feed covers, a new
   * app ID), so this only re-reads the feed to show it.
   */
  protected async patch(change: Partial<ReleasePrefs>): Promise<void> {
    const before = this.draft();
    this.draft.set({ ...before, ...change });
    const stored = await this.ui.guard(this.releases.setPrefs(this.draft()));
    this.draft.set(stored ?? before);
    if (stored !== null) await this.ui.guard(this.releases.refresh());
  }

  protected setWindow(days: number): void {
    void this.patch({ window_days: days });
  }

  protected setAppId(event: Event): void {
    const value = (event.target as HTMLInputElement).value.trim();
    const appId = value === '' ? null : value;
    if (appId === this.draft().qobuz_app_id) return;
    void this.patch({ qobuz_app_id: appId });
  }
}
