import { Component, computed, inject } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import {
  faBackwardStep,
  faForwardStep,
  faPause,
  faPlay,
  faRepeat,
  faShuffle,
  faStop,
  faVolumeUp,
} from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { PlaybackService } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-transport-bar',
  imports: [FaIconComponent],
  templateUrl: './transport-bar.component.html',
})
export class TransportBarComponent {
  protected readonly playback = inject(PlaybackService);
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);

  protected readonly faPlay = faPlay;
  protected readonly faPause = faPause;
  protected readonly faStop = faStop;
  protected readonly faPrev = faBackwardStep;
  protected readonly faNext = faForwardStep;
  protected readonly faShuffle = faShuffle;
  protected readonly faRepeat = faRepeat;
  protected readonly faVolumeUp = faVolumeUp;

  /** O(1) lookup via LibraryService.tracksById — constant-time at any library size. */
  protected readonly currentTrack = computed(this.#computeCurrentTrack.bind(this));

  /**
   * Quality chip pulled from the current track. Returns null when no
   * sample rate is available — bit-depth alone is too thin to justify
   * a chip. Format is displayed elsewhere (the kind chip).
   */
  protected readonly qualityChip = computed(this.#computeQualityChip.bind(this));

  /** Container/codec chip ("FLAC", "MP3"). Falls back to null. */
  protected readonly kindChip = computed(this.#computeKindChip.bind(this));

  #computeCurrentTrack() {
    const id = this.playback.currentTrackId();
    if (id === null) return null;
    return this.library.tracksById().get(id) ?? null;
  }

  #computeQualityChip(): string | null {
    const t = this.currentTrack();
    if (t?.sampleRate == null) return null;
    const khz = (t.sampleRate / 1000).toFixed(t.sampleRate % 1000 === 0 ? 0 : 1);
    if (t.bitDepth == null) return `${khz}k`;
    return `${khz}k/${t.bitDepth}`;
  }

  #computeKindChip(): string | null {
    const t = this.currentTrack();
    if (!t?.kind) return null;
    return t.kind.toUpperCase();
  }

  /**
   * Whether the active source is "lossless-tier" (≥ 24-bit or PCM in a
   * lossless container). Used to color the quality chip — lossless gets
   * the accent color, lossy gets the muted-text color.
   */
  protected isLossless(): boolean {
    const t = this.currentTrack();
    if (!t) return false;
    if (t.bitDepth != null && t.bitDepth >= 24) return true;
    const kind = (t.kind ?? '').toLowerCase();
    return ['flac', 'wav', 'aiff', 'alac'].includes(kind);
  }

  protected async togglePlay(): Promise<void> {
    await this.playback.togglePlay();
  }

  protected async stop(): Promise<void> {
    await this.playback.stop();
  }

  protected async onVolumeInput(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    await this.playback.setVolume(Number(input.value));
  }

  protected async onSeek(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    await this.playback.seek(Number(input.value));
  }

  protected formatTime(ms: number): string {
    return formatMmSs(ms);
  }

  protected toggleNowPlaying(): void {
    this.ui.nowPlayingOpen.update((v) => !v);
  }

  protected async next(): Promise<void> {
    // For v1, "Next" pulls from the user-built queue. Auto-advance on
    // natural track end is wired separately via PlaybackService.
    await this.playback.advanceFromQueue();
  }
}
