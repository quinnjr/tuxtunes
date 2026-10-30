import { Injectable, OnDestroy, computed, inject, signal } from '@angular/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { TauriService } from './tauri.service';

/** One card in the New Releases grid. Mirrors `commands::releases::ReleaseCard`. */
export interface ReleaseCard {
  id: string;
  title: string;
  artist: string;
  /** `YYYY-MM-DD`, or `YYYY-MM` / `YYYY` when MusicBrainz knows no more. */
  releaseDate: string;
  primaryType: string | null;
  secondaryTypes: string[];
  inLibrary: boolean;
  coverUrl: string;
  qobuzUrl: string;
  /** True when `qobuzUrl` is the album itself rather than a search. */
  qobuzExact: boolean;
  musicbrainzUrl: string;
}

interface ReleaseCardRaw {
  id: string;
  title: string;
  artist: string;
  release_date: string;
  primary_type: string | null;
  secondary_types: string[];
  in_library: boolean;
  cover_url: string;
  qobuz_url: string;
  qobuz_exact: boolean;
  musicbrainz_url: string;
}

interface FeedViewRaw {
  checked_at: string | null;
  running: boolean;
  started: boolean;
  window_days: number;
  fetched_window_days: number | null;
  releases: ReleaseCardRaw[];
  last_error: string | null;
  qobuz_error: string | null;
}

export interface FeedView {
  checkedAt: Date | null;
  running: boolean;
  /** False until the user has checked once. */
  started: boolean;
  /** The window the releases shown cover. */
  windowDays: number;
  /** The window the last finished check fetched; null before one has finished. */
  fetchedWindowDays: number | null;
  releases: ReleaseCard[];
  lastError: string | null;
  qobuzError: string | null;
}

/** Mirrors `library::releases::feed::ReleasePrefs`; snake_case crosses IPC as-is. */
export interface ReleasePrefs {
  window_days: number;
  qobuz_app_id: string | null;
}

export const DEFAULT_RELEASE_PREFS: ReleasePrefs = { window_days: 90, qobuz_app_id: null };

export interface ReleaseProgress {
  done: number;
  total: number;
  releases: number;
}

const EMPTY_FEED: FeedView = {
  checkedAt: null,
  running: false,
  started: false,
  windowDays: DEFAULT_RELEASE_PREFS.window_days,
  fetchedWindowDays: null,
  releases: [],
  lastError: null,
  qobuzError: null,
};

function fromRaw(raw: FeedViewRaw): FeedView {
  return {
    checkedAt: raw.checked_at === null ? null : new Date(raw.checked_at),
    running: raw.running,
    started: raw.started,
    windowDays: raw.window_days,
    fetchedWindowDays: raw.fetched_window_days ?? null,
    lastError: raw.last_error,
    qobuzError: raw.qobuz_error,
    releases: raw.releases.map((r) => ({
      id: r.id,
      title: r.title,
      artist: r.artist,
      releaseDate: r.release_date,
      primaryType: r.primary_type,
      secondaryTypes: r.secondary_types,
      inLibrary: r.in_library,
      coverUrl: r.cover_url,
      qobuzUrl: r.qobuz_url,
      qobuzExact: r.qobuz_exact,
      musicbrainzUrl: r.musicbrainz_url,
    })),
  };
}

/** "Album", "Album · Live", "Single". */
export function releaseKind(r: Pick<ReleaseCard, 'primaryType' | 'secondaryTypes'>): string {
  return [r.primaryType, ...r.secondaryTypes].filter((t): t is string => !!t).join(' · ');
}

@Injectable({ providedIn: 'root' })
export class ReleasesService implements OnDestroy {
  private readonly tauri = inject(TauriService);

  readonly feed = signal<FeedView>(EMPTY_FEED);
  /** Set from `releases:progress` while a refresh runs. */
  readonly progress = signal<ReleaseProgress | null>(null);
  /** The run state, as the backend last reported it. */
  readonly running = signal(false);
  /**
   * The error the last run ended with, from `releases:finished`. The
   * backend also stores it in the feed; this covers a run that could
   * not write the file.
   */
  readonly runError = signal<string | null>(null);

  readonly releases = computed(this.#computeReleases.bind(this));

  #computeReleases(): ReleaseCard[] {
    return this.feed().releases;
  }

  private readonly unlisteners: UnlistenFn[] = [];
  /**
   * Bumped per `refresh()`; a reply that is not the latest is dropped,
   * so a snapshot taken mid-run cannot land after the finished event's
   * refresh and put the view back into "Checking…".
   */
  #refreshSeq = 0;

  constructor() {
    void this.subscribe();
  }

  ngOnDestroy(): void {
    for (const off of this.unlisteners) off();
    this.unlisteners.length = 0;
  }

  private async subscribe(): Promise<void> {
    this.unlisteners.push(
      await this.tauri.listen<ReleaseProgress>('releases:progress', (p) => {
        this.running.set(true);
        this.progress.set(p);
      }),
      await this.tauri.listen<null>(
        'releases:updated',
        () => void this.refresh().catch(() => null),
      ),
      await this.tauri.listen<{ error: string | null }>('releases:finished', (p) => {
        this.running.set(false);
        this.runError.set(p.error);
        this.progress.set(null);
        void this.refresh().catch(() => null);
      }),
    );
  }

  /** Re-read the stored feed. Cheap: it is a file read, not a network call. */
  async refresh(): Promise<void> {
    const seq = ++this.#refreshSeq;
    const raw = await this.tauri.invoke<FeedViewRaw>('get_new_releases');
    if (seq !== this.#refreshSeq) return;
    const view = fromRaw(raw);
    this.feed.set(view);
    this.running.set(view.running);
    if (!view.running) this.progress.set(null);
  }

  /** Ask the backend to check MusicBrainz (and Qobuz) now. */
  async checkNow(): Promise<void> {
    // A feed reply already in flight predates this check; drop it, or
    // its running:false would put the idle view back.
    this.#refreshSeq += 1;
    this.running.set(true);
    this.runError.set(null);
    try {
      await this.tauri.invoke<boolean>('check_new_releases');
    } catch (error) {
      this.running.set(false);
      throw error;
    }
  }

  async open(url: string): Promise<void> {
    await this.tauri.invoke('open_release_link', { url });
  }

  async getPrefs(): Promise<ReleasePrefs> {
    return this.tauri.invoke<ReleasePrefs>('get_release_prefs');
  }

  /** Returns what was stored (the window clamped, the app_id trimmed). */
  async setPrefs(prefs: ReleasePrefs): Promise<ReleasePrefs> {
    return this.tauri.invoke<ReleasePrefs>('set_release_prefs', { prefs });
  }
}
