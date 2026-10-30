import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { ContextMenuService } from '../../services/context-menu.service';
import { ReleasesService } from '../../services/releases.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import {
  NewReleasesViewComponent,
  formatCheckedAgo,
  formatReleaseDate,
} from './new-releases-view.component';

const card = (over: Record<string, unknown> = {}) => ({
  id: 'rg1',
  title: 'Moon',
  artist: 'Björk',
  release_date: '2026-10-01',
  primary_type: 'Album',
  secondary_types: [],
  in_library: false,
  cover_url: 'https://coverartarchive.org/release-group/rg1/front-250',
  qobuz_url: 'https://open.qobuz.com/album/q1',
  qobuz_exact: true,
  musicbrainz_url: 'https://musicbrainz.org/release-group/rg1',
  ...over,
});

async function setup(feed: Record<string, unknown>) {
  const stub = tauriStub(async (cmd) => {
    if (cmd === 'get_new_releases') {
      return {
        checked_at: null,
        running: false,
        started: true,
        window_days: 90,
        releases: [],
        last_error: null,
        qobuz_error: null,
        ...feed,
      };
    }
    return defaultInvoke(cmd);
  });
  TestBed.configureTestingModule({
    imports: [NewReleasesViewComponent],
    providers: [...appProviders(stub), ReleasesService],
  });
  const fixture = TestBed.createComponent(NewReleasesViewComponent);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  const el = fixture.nativeElement as HTMLElement;
  return { fixture, el, stub, ui: TestBed.inject(UiService) };
}

describe('NewReleasesViewComponent', () => {
  it('asks before the first check and starts one on click', async () => {
    const { el, stub } = await setup({ started: false });
    expect(el.querySelector('[data-testid="releases-intro"]')).not.toBeNull();
    el.querySelector<HTMLButtonElement>('[data-testid="releases-intro"] button')!.click();
    expect(stub.invoke).toHaveBeenCalledWith('check_new_releases');
  });

  it('says when nothing is new in the window', async () => {
    const { el } = await setup({ window_days: 30, checked_at: '2026-10-30T12:00:00Z' });
    expect(el.textContent).toContain('Nothing new from your artists in the last 30 days');
  });

  it('renders a card per release and opens Qobuz on click', async () => {
    const { el, stub } = await setup({
      releases: [card(), card({ id: 'rg2', title: 'Sun', in_library: true })],
    });
    const items = el.querySelectorAll('[data-testid="releases-grid"] li');
    expect(items.length).toBe(2);
    expect(items[0].textContent).toContain('Moon');
    expect(items[0].textContent).toContain('Oct 1, 2026');
    expect(items[0].textContent).toContain('Album');
    expect(items[1].textContent).toContain('In Library');
    expect(items[0].querySelector('img')?.getAttribute('src')).toBe(card().cover_url);
    items[0].querySelector('button')!.click();
    expect(stub.invoke).toHaveBeenCalledWith('open_release_link', {
      url: 'https://open.qobuz.com/album/q1',
    });
  });

  it('shows a new cover for a card whose old cover was broken', async () => {
    const { el, fixture } = await setup({ releases: [card()] });
    el.querySelector('img')!.dispatchEvent(new Event('error'));
    fixture.detectChanges();
    const releases = TestBed.inject(ReleasesService);
    releases.feed.update((f) => ({
      ...f,
      releases: [{ ...f.releases[0], coverUrl: 'https://static.qobuz.com/x_600.jpg' }],
    }));
    fixture.detectChanges();
    expect(el.querySelector('img')?.getAttribute('src')).toBe('https://static.qobuz.com/x_600.jpg');
  });

  it('swaps a broken cover for the placeholder', async () => {
    const { el, fixture } = await setup({ releases: [card()] });
    el.querySelector('img')!.dispatchEvent(new Event('error'));
    fixture.detectChanges();
    expect(el.querySelector('img')).toBeNull();
    expect(el.textContent).toContain('No art');
  });

  it('offers Qobuz and MusicBrainz in the context menu', async () => {
    const { el } = await setup({ releases: [card({ qobuz_exact: false })] });
    el.querySelector('li')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
    const labels = TestBed.inject(ContextMenuService)
      .open()
      ?.items.map((i) => i.label);
    expect(labels).toEqual(['Search on Qobuz', 'Open on MusicBrainz']);
  });

  it('says no check has finished when the only run failed', async () => {
    const { el } = await setup({ last_error: 'boom' });
    expect(el.textContent).toContain('No check has finished yet');
    expect(el.textContent).not.toContain('Nothing new');
  });

  it('shows the last error and opens its settings tab', async () => {
    const { el, ui } = await setup({ last_error: 'musicbrainz unavailable' });
    expect(el.querySelector('[data-testid="releases-error"]')?.textContent).toContain(
      'musicbrainz unavailable',
    );
    const settings = [...el.querySelectorAll('button')].find((b) =>
      b.textContent?.includes('Settings'),
    )!;
    settings.click();
    expect(ui.settingsOpen()).toBe(true);
    expect(ui.settingsTabRequest()).toBe('releases');
  });
});

describe('formatReleaseDate', () => {
  it('handles full and partial MusicBrainz dates', () => {
    expect(formatReleaseDate('2026-10-03')).toBe('Oct 3, 2026');
    expect(formatReleaseDate('2026-10')).toBe('Oct 2026');
    expect(formatReleaseDate('2026')).toBe('2026');
  });
});

describe('formatCheckedAgo', () => {
  it('counts minutes, hours and days', () => {
    const now = new Date('2026-10-30T12:00:00Z');
    expect(formatCheckedAgo(new Date('2026-10-30T11:59:45Z'), now)).toBe('just now');
    expect(formatCheckedAgo(new Date('2026-10-30T11:55:00Z'), now)).toBe('5 minutes ago');
    expect(formatCheckedAgo(new Date('2026-10-30T11:00:00Z'), now)).toBe('1 hour ago');
    expect(formatCheckedAgo(new Date('2026-10-28T12:00:00Z'), now)).toBe('2 days ago');
  });
});
