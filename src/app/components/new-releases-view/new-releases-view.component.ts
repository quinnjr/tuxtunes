import { Component, OnInit, inject, signal, ChangeDetectionStrategy } from '@angular/core';
import { ContextMenuService } from '../../services/context-menu.service';
import { ReleaseCard, ReleasesService, releaseKind } from '../../services/releases.service';
import { UiService } from '../../services/ui.service';

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/**
 * "Oct 3, 2026" from a MusicBrainz date, which may stop at the month
 * ("Oct 2026") or the year. Parsed by hand: `new Date('2026-10-03')`
 * is UTC midnight, which renders as the 2nd west of Greenwich.
 */
export function formatReleaseDate(date: string): string {
  const [y, m, d] = date.split('-').map((p) => Number.parseInt(p, 10));
  const month = m !== undefined && m >= 1 && m <= 12 ? MONTHS[m - 1] : null;
  if (month === null) return String(y ?? date);
  return d === undefined || Number.isNaN(d) ? `${month} ${y}` : `${month} ${d}, ${y}`;
}

/** "just now", "5 minutes ago", "3 hours ago", "2 days ago". */
export function formatCheckedAgo(at: Date, now: Date = new Date()): string {
  const minutes = Math.floor((now.getTime() - at.getTime()) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'} ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? 'hour' : 'hours'} ago`;
  const days = Math.floor(hours / 24);
  return `${days} ${days === 1 ? 'day' : 'days'} ago`;
}

@Component({
  selector: 'app-new-releases-view',
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './new-releases-view.component.html',
})
export class NewReleasesViewComponent implements OnInit {
  protected readonly releases = inject(ReleasesService);
  private readonly ctx = inject(ContextMenuService);
  private readonly ui = inject(UiService);

  /**
   * Cover URLs that failed to load; their cards show the placeholder.
   * Keyed by URL, not release, so a card whose Cover Art Archive image
   * 404'd shows a Qobuz cover a later check finds.
   */
  protected readonly brokenCovers = signal<ReadonlySet<string>>(new Set());

  protected readonly kind = releaseKind;
  protected readonly formatDate = formatReleaseDate;

  ngOnInit(): void {
    void this.ui.guard(this.releases.refresh());
  }

  protected checkNow(): void {
    void this.ui.guard(this.releases.checkNow());
  }

  protected openSettings(): void {
    this.ui.settingsTabRequest.set('releases');
    this.ui.settingsOpen.set(true);
  }

  protected checkedAgo(at: Date): string {
    return formatCheckedAgo(at);
  }

  protected onCoverError(r: ReleaseCard): void {
    this.brokenCovers.update((s) => new Set(s).add(r.coverUrl));
  }

  protected open(r: ReleaseCard): void {
    void this.ui.guard(this.releases.open(r.qobuzUrl));
  }

  protected onContextMenu(r: ReleaseCard, event: MouseEvent): void {
    this.ctx.show(event, [
      {
        label: r.qobuzExact ? 'Open on Qobuz' : 'Search on Qobuz',
        action: () => this.open(r),
      },
      {
        label: 'Open on MusicBrainz',
        action: () => void this.ui.guard(this.releases.open(r.musicbrainzUrl)),
      },
    ]);
  }

  protected trackById(_index: number, r: ReleaseCard): string {
    return r.id;
  }
}
