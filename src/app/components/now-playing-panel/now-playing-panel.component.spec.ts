import type { CdkDragDrop } from '@angular/cdk/drag-drop';
import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { NowPlayingPanelComponent } from './now-playing-panel.component';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (p: string) => `asset://${p}`,
  invoke: vi.fn(async () => undefined),
}));

interface NowPlayingInternals {
  currentTrack: { (): TrackRow | null };
  onKeydown(e: KeyboardEvent): void;
  close(): void;
  coverUrl(t: TrackRow | null): string | null;
  formatTime(ms: number): string;
  drop(event: CdkDragDrop<TrackRow[]>): void;
  playFromQueue(index: number): Promise<void>;
  advance(): Promise<void>;
  remove(index: number): void;
  clear(): void;
}

const TRACK = (id: number, overrides: Partial<TrackRow> = {}): TrackRow => ({
  id,
  title: `T${id}`,
  artist: null,
  album: null,
  durationMs: 1000,
  filePath: `/tmp/dir/${id}.flac`,
  sampleRate: null,
  bitDepth: null,
  kind: null,
  playCount: 0,
  skipCount: 0,
  ...overrides,
});

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [NowPlayingPanelComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(NowPlayingPanelComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as NowPlayingInternals,
    library: TestBed.inject(LibraryService),
    playback: TestBed.inject(PlaybackService),
    ui: TestBed.inject(UiService),
  };
}

describe('NowPlayingPanelComponent', () => {
  it('currentTrack resolves through tracksById', () => {
    const { cmp, library, playback } = setup();
    expect(cmp.currentTrack()).toBeNull();
    library.tracks.set([TRACK(1)]);
    playback.currentTrackId.set(1);
    expect(cmp.currentTrack()?.id).toBe(1);
  });

  it('formatTime delegates to formatMmSs', () => {
    const { cmp } = setup();
    expect(cmp.formatTime(125_000)).toBe('2:05');
  });

  it('coverUrl returns null when no track and asset URL otherwise', () => {
    const { cmp } = setup();
    expect(cmp.coverUrl(null)).toBeNull();
    expect(cmp.coverUrl(TRACK(1, { filePath: '/tmp/a/b.flac' }))).toBe('asset:///tmp/a/cover.jpg');
  });

  it('close() sets nowPlayingOpen to false', () => {
    const { cmp, ui } = setup();
    ui.nowPlayingOpen.set(true);
    cmp.close();
    expect(ui.nowPlayingOpen()).toBe(false);
  });

  it('Q toggles the panel; modifier keys are ignored', () => {
    const { cmp, ui } = setup();
    cmp.onKeydown({
      key: 'q',
      target: document.body,
      preventDefault: vi.fn(),
    } as unknown as KeyboardEvent);
    expect(ui.nowPlayingOpen()).toBe(true);
    cmp.onKeydown({
      key: 'Q',
      target: document.body,
      preventDefault: vi.fn(),
    } as unknown as KeyboardEvent);
    expect(ui.nowPlayingOpen()).toBe(false);
    // Ctrl/meta/alt with Q should NOT toggle.
    cmp.onKeydown({
      key: 'q',
      ctrlKey: true,
      target: document.body,
      preventDefault: vi.fn(),
    } as unknown as KeyboardEvent);
    expect(ui.nowPlayingOpen()).toBe(false);
  });

  it('Q is suppressed when an input is focused', () => {
    const { cmp, ui } = setup();
    const input = document.createElement('input');
    cmp.onKeydown({ key: 'q', target: input, preventDefault: vi.fn() } as unknown as KeyboardEvent);
    expect(ui.nowPlayingOpen()).toBe(false);
  });

  it('non-Q keys are ignored', () => {
    const { cmp, ui } = setup();
    cmp.onKeydown({
      key: 'a',
      target: document.body,
      preventDefault: vi.fn(),
    } as unknown as KeyboardEvent);
    expect(ui.nowPlayingOpen()).toBe(false);
  });

  it('drop reorders the queue via moveItemInArray', () => {
    const { cmp, playback } = setup();
    playback.queue.set([TRACK(1), TRACK(2), TRACK(3)]);
    cmp.drop({ previousIndex: 0, currentIndex: 2 } as CdkDragDrop<TrackRow[]>);
    expect(playback.queue().map((t) => t.id)).toEqual([2, 3, 1]);
  });

  it('playFromQueue removes the entry then plays it', async () => {
    const { cmp, playback } = setup();
    playback.queue.set([TRACK(1), TRACK(2)]);
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.playFromQueue(1);
    expect(playSpy).toHaveBeenCalledWith(2);
    expect(playback.queue().map((t) => t.id)).toEqual([1]);
  });

  it('playFromQueue is a no-op when the index is out of range', async () => {
    const { cmp, playback } = setup();
    playback.queue.set([]);
    const playSpy = vi.spyOn(playback, 'play').mockResolvedValue();
    await cmp.playFromQueue(0);
    expect(playSpy).not.toHaveBeenCalled();
  });

  it('advance / remove / clear forward to PlaybackService', async () => {
    const { cmp, playback } = setup();
    const adv = vi.spyOn(playback, 'advanceFromQueue').mockResolvedValue(null);
    const rem = vi.spyOn(playback, 'removeFromQueue');
    const clr = vi.spyOn(playback, 'clearQueue');
    await cmp.advance();
    cmp.remove(2);
    cmp.clear();
    expect(adv).toHaveBeenCalled();
    expect(rem).toHaveBeenCalledWith(2);
    expect(clr).toHaveBeenCalled();
  });
});
