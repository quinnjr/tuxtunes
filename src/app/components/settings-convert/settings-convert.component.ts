import { Component, OnInit, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import {
  ConvertPrefs,
  ConvertService,
  DEFAULT_CONVERT_PREFS,
  M4aCodec,
} from '../../services/convert.service';
import { UiService } from '../../services/ui.service';
import { Choice, ChoiceSelectComponent } from '../choice-select/choice-select.component';
import { ConvertActivityComponent } from '../convert-activity/convert-activity.component';

const SAMPLE_RATES: readonly Choice<number | null>[] = [
  { value: null, label: 'Same as source' },
  { value: 44_100, label: '44.1 kHz' },
  { value: 48_000, label: '48 kHz' },
  { value: 88_200, label: '88.2 kHz' },
  { value: 96_000, label: '96 kHz' },
  { value: 176_400, label: '176.4 kHz' },
  { value: 192_000, label: '192 kHz' },
];

/** AAC's MPEG-4 table stops at 96 kHz; the encoder rejects anything above. */
const AAC_SAMPLE_RATES: readonly Choice<number | null>[] = SAMPLE_RATES.filter(
  (r) => r.value === null || r.value <= 96_000,
);

@Component({
  selector: 'app-settings-convert',
  imports: [ChoiceSelectComponent, ConvertActivityComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './settings-convert.component.html',
})
export class SettingsConvertComponent implements OnInit {
  protected readonly convert = inject(ConvertService);
  private readonly ui = inject(UiService);

  /** Editable copy of the stored prefs; every edit persists immediately. */
  protected readonly draft = signal<ConvertPrefs>(DEFAULT_CONVERT_PREFS);
  /**
   * False until the stored prefs have been read. The form is disabled
   * meanwhile: an edit made against the defaults would be saved as the
   * whole blob and silently replace everything the user had set.
   */
  protected readonly loaded = signal(false);

  protected readonly codecs: readonly Choice<M4aCodec>[] = [
    { value: 'alac', label: 'ALAC (lossless)' },
    { value: 'aac', label: 'AAC (lossy)' },
  ] as const;

  protected readonly sampleRates = SAMPLE_RATES;
  protected readonly aacSampleRates = AAC_SAMPLE_RATES;

  protected readonly flacDepths: readonly Choice<number | null>[] = [
    { value: null, label: 'Same as source' },
    { value: 16, label: '16-bit' },
    { value: 24, label: '24-bit' },
    { value: 32, label: '32-bit' },
  ] as const;

  /** ALAC has no 32-bit mode, so its depth list stops at 24. */
  protected readonly alacDepths: readonly Choice<number | null>[] = [
    { value: null, label: 'Same as source' },
    { value: 16, label: '16-bit' },
    { value: 24, label: '24-bit' },
  ] as const;

  protected readonly aacBitrates: readonly Choice<number>[] = [
    { value: 96, label: '96 kbps' },
    { value: 128, label: '128 kbps' },
    { value: 160, label: '160 kbps' },
    { value: 192, label: '192 kbps' },
    { value: 256, label: '256 kbps' },
    { value: 320, label: '320 kbps' },
  ] as const;

  ngOnInit(): void {
    void this.reload();
  }

  protected async reload(): Promise<void> {
    const ok = await this.ui.guard(this.convert.refresh());
    if (ok === null) return;
    this.draft.set(this.convert.prefs());
    this.loaded.set(true);
  }

  /**
   * Apply a patch to the draft and persist it. The backend clamps the
   * values and returns what it stored, so the draft re-seeds from this
   * call's own reply — an out-of-range number typed into a box snaps
   * back visibly. A failed save rolls the draft back, so the form never
   * claims a setting the backend does not have.
   */
  protected async patch(change: Partial<ConvertPrefs>): Promise<void> {
    const before = this.draft();
    const next = { ...before, ...change };
    this.draft.set(next);
    const stored = await this.ui.guard(this.convert.savePrefs(next));
    this.draft.set(stored ?? before);
  }

  protected patchFlac(change: Partial<ConvertPrefs['flac']>): void {
    void this.patch({ flac: { ...this.draft().flac, ...change } });
  }

  protected patchM4a(change: Partial<ConvertPrefs['m4a']>): void {
    void this.patch({ m4a: { ...this.draft().m4a, ...change } });
  }

  protected async pickOutputDir(): Promise<void> {
    const picked = await this.ui.pickDirectory();
    if (picked !== null) void this.patch({ output_dir: picked });
  }

  protected resetDefaults(): void {
    void this.patch(DEFAULT_CONVERT_PREFS);
  }
}
