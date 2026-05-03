import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, type TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { TransportBarComponent } from './transport-bar.component';

interface TransportInternals {
  togglePlay(): Promise<void>;
  stop(): Promise<void>;
  next(): Promise<void>;
  onVolumeInput(event: Event): Promise<void>;
  onSeek(event: Event): Promise<void>;
  formatTime(ms: number): string;
  toggleNowPlaying(): void;
  qualityChip: { (): string | null };
  kindChip: { (): string | null };
  isLossless(): boolean;
  currentTrack: { (): TrackRow | null };
}

const TRACK: TrackRow = {
  id: 1,
  title: 'T',
  artist: 'A',
  album: 'Al',
  durationMs: 180_000,
  filePath: '/tmp/a.flac',
  sampleRate: 96_000,
  bitDepth: 24,
  kind: 'flac',
  playCount: 0,
  skipCount: 0,
};

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [TransportBarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(TransportBarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as TransportInternals,
    library: TestBed.inject(LibraryService),
    playback: TestBed.inject(PlaybackService),
    ui: TestBed.inject(UiService),
  };
}

describe('TransportBarComponent', () => {
  it('currentTrack() resolves through tracksById', () => {
    const { cmp, library, playback } = setup();
    expect(cmp.currentTrack()).toBeNull();
    library.tracks.set([TRACK]);
    playback.currentTrackId.set(TRACK.id);
    expect(cmp.currentTrack()?.id).toBe(TRACK.id);
  });

  it('qualityChip + kindChip compute from the current track', () => {
    const { cmp, library, playback } = setup();
    library.tracks.set([TRACK]);
    playback.currentTrackId.set(TRACK.id);
    expect(cmp.qualityChip()).toBe('96k/24');
    expect(cmp.kindChip()).toBe('FLAC');
  });

  it('qualityChip handles non-integer kHz with one decimal', () => {
    const { cmp, library, playback } = setup();
    library.tracks.set([{ ...TRACK, sampleRate: 88_200, bitDepth: null }]);
    playback.currentTrackId.set(TRACK.id);
    expect(cmp.qualityChip()).toBe('88.2k');
  });

  it('chips return null when no track is loaded', () => {
    const { cmp } = setup();
    expect(cmp.qualityChip()).toBeNull();
    expect(cmp.kindChip()).toBeNull();
  });

  it('chips return null when sample rate / kind metadata is missing', () => {
    const { cmp, library, playback } = setup();
    library.tracks.set([{ ...TRACK, sampleRate: null, kind: null }]);
    playback.currentTrackId.set(TRACK.id);
    expect(cmp.qualityChip()).toBeNull();
    expect(cmp.kindChip()).toBeNull();
  });

  it('isLossless returns true for ≥24-bit + lossless containers, false otherwise', () => {
    const { cmp, library, playback } = setup();
    library.tracks.set([{ ...TRACK, bitDepth: 24, kind: 'mp3' }]);
    playback.currentTrackId.set(TRACK.id);
    expect(cmp.isLossless()).toBe(true);

    library.tracks.set([{ ...TRACK, bitDepth: 16, kind: 'wav' }]);
    expect(cmp.isLossless()).toBe(true);

    library.tracks.set([{ ...TRACK, bitDepth: 16, kind: 'mp3' }]);
    expect(cmp.isLossless()).toBe(false);
  });

  it('isLossless returns false when no track is loaded', () => {
    const { cmp } = setup();
    expect(cmp.isLossless()).toBe(false);
  });

  it('togglePlay() / stop() / next() forward to PlaybackService', async () => {
    const { cmp, playback } = setup();
    const t = vi.spyOn(playback, 'togglePlay').mockResolvedValue();
    const s = vi.spyOn(playback, 'stop').mockResolvedValue();
    const a = vi.spyOn(playback, 'advanceFromQueue').mockResolvedValue(null);
    await cmp.togglePlay();
    await cmp.stop();
    await cmp.next();
    expect(t).toHaveBeenCalled();
    expect(s).toHaveBeenCalled();
    expect(a).toHaveBeenCalled();
  });

  it('onVolumeInput / onSeek read input value and forward', async () => {
    const { cmp, playback } = setup();
    const sv = vi.spyOn(playback, 'setVolume').mockResolvedValue();
    const sk = vi.spyOn(playback, 'seek').mockResolvedValue();
    await cmp.onVolumeInput({ target: { value: '42' } } as unknown as Event);
    await cmp.onSeek({ target: { value: '7000' } } as unknown as Event);
    expect(sv).toHaveBeenCalledWith(42);
    expect(sk).toHaveBeenCalledWith(7000);
  });

  it('formatTime() pads seconds', () => {
    const { cmp } = setup();
    expect(cmp.formatTime(65_000)).toBe('1:05');
  });

  it('toggleNowPlaying() flips the UI signal', () => {
    const { cmp, ui } = setup();
    expect(ui.nowPlayingOpen()).toBe(false);
    cmp.toggleNowPlaying();
    expect(ui.nowPlayingOpen()).toBe(true);
    cmp.toggleNowPlaying();
    expect(ui.nowPlayingOpen()).toBe(false);
  });
});
