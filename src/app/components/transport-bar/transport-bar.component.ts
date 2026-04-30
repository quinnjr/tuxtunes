import { Component, computed, inject, signal } from '@angular/core';
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
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-transport-bar',
  imports: [FaIconComponent],
  templateUrl: './transport-bar.component.html',
})
export class TransportBarComponent {
  protected readonly playback = inject(PlaybackService);
  private readonly library = inject(LibraryService);

  protected readonly faPlay = faPlay;
  protected readonly faPause = faPause;
  protected readonly faStop = faStop;
  protected readonly faPrev = faBackwardStep;
  protected readonly faNext = faForwardStep;
  protected readonly faShuffle = faShuffle;
  protected readonly faRepeat = faRepeat;
  protected readonly faVolumeUp = faVolumeUp;

  protected readonly volume = signal<number>(100);

  protected readonly currentTrack = computed(this.#computeCurrentTrack.bind(this));

  #computeCurrentTrack() {
    const id = this.playback.currentTrackId();
    if (id === null) return null;
    const tracks = this.library.tracks();
    for (const track of tracks) {
      if (track.id === id) return track;
    }
    return null;
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
    const value = Number(input.value);
    this.volume.set(value);
    await this.playback.setVolume(value);
  }

  protected async onSeek(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    await this.playback.seek(Number(input.value));
  }

  protected formatTime(ms: number): string {
    return formatMmSs(ms);
  }
}
