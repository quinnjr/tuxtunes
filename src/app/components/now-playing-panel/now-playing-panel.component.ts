import { CdkDrag, CdkDragDrop, CdkDropList, moveItemInArray } from '@angular/cdk/drag-drop';
import { Component, HostListener, computed, inject } from '@angular/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-now-playing-panel',
  imports: [CdkDropList, CdkDrag],
  templateUrl: './now-playing-panel.component.html',
})
export class NowPlayingPanelComponent {
  protected readonly playback = inject(PlaybackService);
  private readonly library = inject(LibraryService);
  protected readonly ui = inject(UiService);

  protected readonly currentTrack = computed(this.#computeCurrentTrack.bind(this));

  #computeCurrentTrack(): TrackRow | null {
    const id = this.playback.currentTrackId();
    if (id === null) return null;
    return this.library.tracksById().get(id) ?? null;
  }

  /**
   * Q toggles the panel. The HostListener attaches to document, so the
   * shortcut works no matter which child element has focus.
   */
  @HostListener('document:keydown', ['$event'])
  onKeydown(event: KeyboardEvent): void {
    if (event.key !== 'q' && event.key !== 'Q') return;
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target as HTMLElement | null;
    // Don't hijack Q while the user is typing in an input.
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
    this.ui.nowPlayingOpen.update((v) => !v);
    event.preventDefault();
  }

  protected close(): void {
    this.ui.nowPlayingOpen.set(false);
  }

  protected coverUrl(track: TrackRow | null): string | null {
    if (!track) return null;
    // Phase 4's ingest stores artwork as cover.<ext> next to the
    // track file; the backend exposes it via Track.artwork_path. The
    // current TrackRow shape doesn't carry that, so fall back to a
    // sibling cover.jpg from the file path.
    const dir = track.filePath.replace(/\/[^/]+$/, '');
    return convertFileSrc(`${dir}/cover.jpg`);
  }

  protected formatTime(ms: number): string {
    return formatMmSs(ms);
  }

  protected drop(event: CdkDragDrop<TrackRow[]>): void {
    const next = [...this.playback.queue()];
    moveItemInArray(next, event.previousIndex, event.currentIndex);
    this.playback.queue.set(next);
  }

  protected async playFromQueue(index: number): Promise<void> {
    const track = this.playback.queue()[index];
    if (!track) return;
    this.playback.removeFromQueue(index);
    await this.playback.play(track.id);
  }

  protected async advance(): Promise<void> {
    await this.playback.advanceFromQueue();
  }

  protected remove(index: number): void {
    this.playback.removeFromQueue(index);
  }

  protected clear(): void {
    this.playback.clearQueue();
  }
}
