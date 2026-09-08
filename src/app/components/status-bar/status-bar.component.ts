import { Component, computed, inject, ChangeDetectionStrategy } from '@angular/core';
import { ConvertService } from '../../services/convert.service';
import { LibraryService } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { UiService } from '../../services/ui.service';
import { formatByteSize, formatTotalDuration } from '../../utils/format';

@Component({
  selector: 'app-status-bar',
  imports: [],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './status-bar.component.html',
})
export class StatusBarComponent {
  protected readonly convert = inject(ConvertService);
  protected readonly library = inject(LibraryService);
  protected readonly sync = inject(SyncService);
  protected readonly ui = inject(UiService);

  protected readonly summary = computed(this.#computeSummary.bind(this));
  protected readonly activityLabel = computed(this.#computeActivityLabel.bind(this));

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

  #computeActivityLabel(): string | null {
    // Conversion is started from a context menu anywhere in the app, so
    // the status bar is the only place its progress is guaranteed to be
    // visible. It outranks the sync label because the user just asked
    // for it.
    const convert = this.convert.progress();
    if (convert && this.convert.running()) {
      return `Converting ${convert.current + 1} of ${convert.total}…`;
    }
    const state = this.sync.runState();
    if (state === 'running') return 'Syncing…';
    if (state === 'error') return 'Sync error';
    return null;
  }
}
