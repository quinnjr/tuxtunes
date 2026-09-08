import {
  Component,
  OnInit,
  computed,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
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

  protected readonly summary = computed(this.computeSummary.bind(this));

  ngOnInit(): void {
    void this.ui.guard(this.convert.refresh()).then((ok) => {
      if (ok !== null) this.draft.set(this.convert.prefs());
    });
  }

  /**
   * Apply a patch to the draft and persist it. The backend clamps the
   * values and returns what it stored, so the draft re-seeds from that
   * — an out-of-range number typed into a box snaps back visibly.
   */
  protected async patch(change: Partial<ConvertPrefs>): Promise<void> {
    const next = { ...this.draft(), ...change };
    this.draft.set(next);
    const ok = await this.ui.guard(this.convert.savePrefs(next));
    if (ok !== null) this.draft.set(this.convert.prefs());
  }

  protected patchFlac(change: Partial<ConvertPrefs['flac']>): void {
    void this.patch({ flac: { ...this.draft().flac, ...change } });
  }

  protected patchM4a(change: Partial<ConvertPrefs['m4a']>): void {
    void this.patch({ m4a: { ...this.draft().m4a, ...change } });
  }

  protected async pickOutputDir(): Promise<void> {
    const picked = await this.ui.guard(dialogOpen({ directory: true, multiple: false }));
    if (typeof picked === 'string') void this.patch({ output_dir: picked });
  }

  protected resetDefaults(): void {
    void this.patch(DEFAULT_CONVERT_PREFS);
  }

  private computeSummary(): string {
    const c = this.convert.lastComplete();
    if (!c) return '';
    // A run of mixed formats has no single name to give.
    const to = c.format.includes('+') ? '' : ` to ${c.format.toUpperCase()}`;
    const parts = [`${c.converted} converted${to}`];
    if (c.addedToLibrary > 0) parts.push(`${c.addedToLibrary} added to the library`);
    if (c.failed > 0) parts.push(`${c.failed} failed`);
    if (c.cancelled) parts.push(`cancelled with ${c.total - c.converted - c.failed} left`);
    return parts.join(', ');
  }
}
