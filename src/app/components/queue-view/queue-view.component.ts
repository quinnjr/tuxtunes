import { Component, inject, ChangeDetectionStrategy } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faTriangleExclamation } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-queue-view',
  imports: [FaIconComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './queue-view.component.html',
})
export class QueueViewComponent {
  protected readonly playback = inject(PlaybackService);
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);

  /** Warn glyph for a track whose file is not on disk. */
  protected readonly faTriangleExclamation = faTriangleExclamation;

  protected formatTime(ms: number): string {
    return formatMmSs(ms);
  }

  protected async playFromQueue(index: number): Promise<void> {
    const track = this.playback.queue()[index];
    if (!track) return;
    this.playback.removeFromQueue(index);
    await this.playback.play(track.id);
  }

  protected move(index: number, delta: -1 | 1): void {
    this.playback.reorderQueue(index, index + delta);
  }

  protected remove(index: number): void {
    this.playback.removeFromQueue(index);
  }

  protected clear(): void {
    this.playback.clearQueue();
  }

  protected isCurrent(t: TrackRow): boolean {
    return this.playback.currentTrackId() === t.id;
  }

  /** Tooltip explaining a dimmed row; null for healthy rows. */
  protected rowTitle(t: TrackRow): string | null {
    return t.missing ? `File not found: ${t.filePath}` : null;
  }

  /**
   * Save the live queue as a regular playlist. Reuses the same
   * create-add-refresh flow as "New Playlist…" from a track selection.
   */
  protected saveAsPlaylist(): void {
    const ids = this.playback.queue().map((t) => t.id);
    if (ids.length === 0) return;
    this.ui.namePrompt.set({
      title: 'Save Queue as Playlist',
      initial: '',
      onSubmit: async (name) => {
        await this.ui.guard(this.library.createPlaylistWithTracks(name, ids));
      },
    });
  }
}
