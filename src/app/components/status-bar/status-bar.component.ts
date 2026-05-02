import { Component, computed, inject } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { formatByteSize, formatTotalDuration } from '../../utils/format';

@Component({
  selector: 'app-status-bar',
  imports: [],
  templateUrl: './status-bar.component.html',
})
export class StatusBarComponent {
  protected readonly library = inject(LibraryService);
  protected readonly sync = inject(SyncService);

  protected readonly summary = computed(this.#computeSummary.bind(this));
  protected readonly syncLabel = computed(this.#computeSyncLabel.bind(this));

  #computeSummary() {
    const stats = this.library.stats();
    if (!stats) return null;
    const songsLabel = stats.trackCount === 1 ? 'song' : 'songs';
    return {
      songs: `${stats.trackCount.toLocaleString()} ${songsLabel}`,
      duration: formatTotalDuration(stats.totalDurationMs),
      size: formatByteSize(stats.totalSizeBytes),
    };
  }

  #computeSyncLabel(): string | null {
    const state = this.sync.runState();
    if (state === 'running') return 'Syncing…';
    if (state === 'error') return 'Sync error';
    return null;
  }
}
