import { Component, inject, ChangeDetectionStrategy } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faTriangleExclamation } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { formatMmSs } from '../../utils/time';
import { isCurrentTrack, trackRowTitle } from '../../utils/track-row';

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

  protected onRowActivate(event: Event, index: number): void {
    if ((event.target as HTMLElement | null)?.closest('button')) return;
    void this.playFromQueue(index);
  }

  protected async playFromQueue(index: number): Promise<void> {
    const track = this.playback.queue()[index];
    if (!track) return;
    const ok = await this.playback.play(track.id);
    if (!ok) return;
    // The queue may have shifted during the await; remove by identity
    // when the index still lines up, else fall back to id lookup.
    const q = this.playback.queue();
    const idx = q[index]?.id === track.id ? index : q.findIndex((t) => t.id === track.id);
    if (idx !== -1) this.playback.removeFromQueue(idx);
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

  protected readonly rowBtn =
    'mac-btn h-6 w-6 text-text-muted opacity-0 transition hover:text-accent-text group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100';

  protected rowClass(t: TrackRow): string {
    return (
      'group flex h-[30px] cursor-pointer items-center gap-1 px-4 text-body hover:bg-bg-elevated ' +
      (t.missing ? 'opacity-50' : '')
    );
  }

  protected isCurrent(t: TrackRow): boolean {
    return isCurrentTrack(this.playback, t);
  }

  /** Tooltip explaining a dimmed row; null for healthy rows. */
  protected rowTitle(t: TrackRow): string | null {
    return trackRowTitle(t);
  }

  /**
   * Save the live queue as a regular playlist. Reuses the same
   * create-add-refresh flow as "New Playlist…" from a track selection.
   */
  protected saveAsPlaylist(): void {
    if (this.playback.queue().length === 0) return;
    this.ui.namePrompt.set({
      title: 'Save Queue as Playlist',
      initial: '',
      onSubmit: async (name) => {
        // Snapshot at submit time: the user may have queued, removed,
        // or reordered tracks while the prompt was open.
        const ids = this.playback.queue().map((t) => t.id);
        if (ids.length === 0) return;
        await this.ui.guard(this.library.createPlaylistWithTracks(name, ids));
      },
    });
  }
}
