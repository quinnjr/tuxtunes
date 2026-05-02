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

  #computeCurrentTrack() {
    const id = this.playback.currentTrackId();
    if (id === null) return null;
    return this.library.tracksById().get(id) ?? null;
  }

  protected async togglePlay(): Promise<void> {
    switch (this.playback.state()) {
      case 'playing': {
        await this.playback.pause();
        break;
      }
      case 'paused': {
        await this.playback.resume();
        break;
      }
      default: {
        break;
      }
    }
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
