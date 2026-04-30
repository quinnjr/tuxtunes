import { ScrollingModule } from '@angular/cdk/scrolling';
import { Component, OnInit, inject } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';

@Component({
  selector: 'app-track-list-view',
  imports: [ScrollingModule],
  templateUrl: './track-list-view.component.html',
})
export class TrackListViewComponent implements OnInit {
  protected readonly library = inject(LibraryService);
  protected readonly playback = inject(PlaybackService);

  ngOnInit(): void {
    void this.library.refreshTracks();
  }

  protected async play(track: TrackRow): Promise<void> {
    await this.playback.play(track.id);
  }

  protected trackById = (_: number, row: TrackRow): number => row.id;

  protected formatDuration(ms: number): string {
    const total = Math.round(ms / 1000);
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m}:${s.toString().padStart(2, '0')}`;
  }
}
