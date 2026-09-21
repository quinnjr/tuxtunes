import { CdkDrag, CdkDragDrop, CdkDropList } from '@angular/cdk/drag-drop';
import {
  Component,
  HostListener,
  computed,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { formatMmSs } from '../../utils/time';

@Component({
  selector: 'app-now-playing-panel',
  imports: [CdkDropList, CdkDrag],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './now-playing-panel.component.html',
})
export class NowPlayingPanelComponent {
  protected readonly playback = inject(PlaybackService);
  private readonly library = inject(LibraryService);
  protected readonly ui = inject(UiService);

  protected readonly currentTrack = computed(this.#computeCurrentTrack.bind(this));

  /**
   * Set when the current track's cover fails to load, so the `<img>` hides
   * without being removed. Reset whenever the track changes — otherwise the
   * reused `<img>` element would stay hidden for the next cover.
   */
  protected readonly coverFailed = signal(false);

  constructor() {
    // A new track gets a fresh chance at its cover.
    effect(() => {
      this.coverUrl(this.currentTrack());
      this.coverFailed.set(false);
    });
  }

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

  /** Asset URL for the current track's cached cover, or null. */
  protected coverUrl(track: TrackRow | null): string | null {
    if (!track?.artworkPath) return null;
    return convertFileSrc(track.artworkPath);
  }

  /**
   * Mark the current cover as failed; the template hides the `<img>` and
   * the placeholder tile behind it shows through.
   */
  protected onCoverError(): void {
    this.coverFailed.set(true);
  }

  protected formatTime(ms: number): string {
    return formatMmSs(ms);
  }

  protected drop(event: CdkDragDrop<TrackRow[]>): void {
    this.playback.reorderQueue(event.previousIndex, event.currentIndex);
  }

  protected async playFromQueue(index: number): Promise<void> {
    const track = this.playback.queue()[index];
    if (!track) return;
    this.playback.removeFromQueue(index);
    await this.playback.play(track.id);
  }

  protected async advance(): Promise<void> {
    await this.playback.next();
  }

  protected remove(index: number): void {
    this.playback.removeFromQueue(index);
  }

  protected clear(): void {
    this.playback.clearQueue();
  }
}
