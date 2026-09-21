import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { PlaybackService, TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { QueueViewComponent } from './queue-view.component';

function track(over: Partial<TrackRow> = {}): TrackRow {
  return {
    id: 1,
    title: 'Song',
    artist: 'Artist',
    album: 'Album',
    albumArtist: null,
    genre: null,
    year: null,
    trackNumber: null,
    discNumber: null,
    durationMs: 180_000,
    filePath: '/music/song.flac',
    sampleRate: null,
    bitDepth: null,
    kind: null,
    playCount: 0,
    skipCount: 0,
    missing: false,
    artworkPath: null,
    rating: 0,
    albumRating: 0,
    dateAdded: null,
    ...over,
  };
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [QueueViewComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(QueueViewComponent);
  fixture.detectChanges();
  return {
    fixture,
    playback: TestBed.inject(PlaybackService),
    library: TestBed.inject(LibraryService),
    ui: TestBed.inject(UiService),
    stub,
  };
}

describe('QueueViewComponent', () => {
  it('shows an empty state explaining how to load songs', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('Queue is empty');
  });

  it('renders queued tracks from the live playback queue', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('A');
    expect(text).toContain('B');
  });

  it('Save as Playlist prompts for a name and creates a playlist from queue ids', async () => {
    const { fixture, playback, ui, stub } = setup();
    playback.enqueueAll([track({ id: 7, title: 'A' }), track({ id: 9, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('[data-testid="save-queue"]')!.click();
    expect(ui.namePrompt()?.title).toBe('Save Queue as Playlist');
    stub.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'create_playlist') return 42;
      if (cmd === 'list_playlists') return [];
      const { defaultInvoke } = await import('../../test-helpers');
      return defaultInvoke(cmd);
    });
    await ui.namePrompt()!.onSubmit('Roadtrip');
    expect(stub.invoke).toHaveBeenCalledWith('create_playlist', {
      name: 'Roadtrip',
      parentId: null,
    });
    expect(stub.invoke).toHaveBeenCalledWith('add_tracks_to_playlist', {
      playlistId: 42,
      trackIds: [7, 9],
    });
  });

  it('Clear empties the live queue', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1 })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('[data-testid="clear-queue"]')!.click();
    expect(playback.queue()).toHaveLength(0);
  });

  it('removing a row drops only that track from the live queue', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('[aria-label="Remove from queue"]')!.click();
    expect(playback.queue().map((t) => t.id)).toEqual([2]);
  });

  it('move down reorders the live queue', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('[aria-label="Move down"]')!.click();
    expect(playback.queue().map((t) => t.id)).toEqual([2, 1]);
  });

  it('move up reorders the live queue', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelectorAll<HTMLButtonElement>('[aria-label="Move up"]')[1].click();
    expect(playback.queue().map((t) => t.id)).toEqual([2, 1]);
  });

  it('edge rows disable the out-of-range move button', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelectorAll<HTMLButtonElement>('[aria-label="Move up"]')[0].disabled).toBe(true);
    expect(el.querySelectorAll<HTMLButtonElement>('[aria-label="Move down"]')[1].disabled).toBe(
      true,
    );
  });

  it('duplicate ids render as distinct rows', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 1, title: 'A' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelectorAll('[data-queue-row]').length).toBe(2);
  });

  it('Enter on a row plays that track and consumes it from the queue', async () => {
    const { fixture, playback, stub } = setup();
    playback.enqueueAll([track({ id: 4, title: 'A' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLElement>('[data-queue-row]')!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
    );
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('play_track', { trackId: 4 });
    expect(playback.queue()).toHaveLength(0);
  });

  it('double-click plays the row', async () => {
    const { fixture, playback, stub } = setup();
    playback.enqueueAll([track({ id: 5, title: 'A' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLElement>('[data-queue-row]')!.dispatchEvent(
      new MouseEvent('dblclick', { bubbles: true }),
    );
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('play_track', { trackId: 5 });
  });

  it('Enter on a row button does not start playback', async () => {
    const { fixture, playback, stub } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    stub.invoke.mockClear();
    el.querySelector<HTMLButtonElement>('[aria-label="Move down"]')!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
    );
    await fixture.whenStable();
    expect(stub.invoke).not.toHaveBeenCalledWith('play_track', expect.anything());
  });

  it('Space on a row does not start playback', async () => {
    const { fixture, playback, stub } = setup();
    playback.enqueueAll([track({ id: 4, title: 'A' })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    stub.invoke.mockClear();
    el.querySelector<HTMLElement>('[data-queue-row]')!.dispatchEvent(
      new KeyboardEvent('keydown', { key: ' ', bubbles: true }),
    );
    await fixture.whenStable();
    expect(stub.invoke).not.toHaveBeenCalledWith('play_track', expect.anything());
  });

  it('failed play keeps the track and surfaces the error', async () => {
    const { fixture, playback, stub, ui } = setup();
    playback.enqueueAll([track({ id: 4, title: 'A' })]);
    fixture.detectChanges();
    stub.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'play_track') throw new Error('File not found: /music/song.flac');
      const { defaultInvoke } = await import('../../test-helpers');
      return defaultInvoke(cmd);
    });
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLElement>('[data-queue-row]')!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
    );
    await fixture.whenStable();
    expect(playback.queue().map((t) => t.id)).toEqual([4]);
    expect(ui.lastError()).toContain('File not found');
  });

  it('stale out-of-range activation does not invoke play', async () => {
    const { fixture, playback, stub } = setup();
    playback.enqueueAll([track({ id: 4 })]);
    fixture.detectChanges();
    playback.clearQueue();
    stub.invoke.mockClear();
    const cmp = fixture.componentInstance as unknown as {
      playFromQueue: (i: number) => Promise<void>;
    };
    await cmp.playFromQueue(0);
    expect(stub.invoke).not.toHaveBeenCalledWith('play_track', expect.anything());
    expect(playback.queue()).toEqual([]);
  });

  it('empty queue disables Save and hides Clear without prompting', () => {
    const { fixture, ui, stub } = setup();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelector<HTMLButtonElement>('[data-testid="save-queue"]')!.disabled).toBe(true);
    expect(el.querySelector('[data-testid="clear-queue"]')).toBeNull();
    const cmp = fixture.componentInstance as unknown as { saveAsPlaylist: () => void };
    cmp.saveAsPlaylist();
    expect(ui.namePrompt()).toBeNull();
    expect(stub.invoke).not.toHaveBeenCalledWith('create_playlist', expect.anything());
  });

  it('Save as Playlist reports backend failure instead of rejecting', async () => {
    const { fixture, playback, ui, stub } = setup();
    playback.enqueueAll([track({ id: 7 })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('[data-testid="save-queue"]')!.click();
    stub.invoke.mockRejectedValue(new Error('boom'));
    await ui.namePrompt()!.onSubmit('Roadtrip');
    expect(ui.lastError()).toContain('boom');
  });

  it('highlights the current track with ♫', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'A' }), track({ id: 2, title: 'B' })]);
    playback.currentTrackId.set(2);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    const rows = el.querySelectorAll('[data-queue-row]');
    expect(rows[1].textContent).toContain('♫');
    expect(rows[0].textContent).not.toContain('♫');
  });

  it('missing rows are dimmed with a tooltip, not color alone', () => {
    const { fixture, playback } = setup();
    playback.enqueueAll([track({ id: 1, title: 'Gone', missing: true })]);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    const row = el.querySelector('[data-queue-row]')!;
    expect(row.className).toContain('opacity-50');
    expect(row.getAttribute('title')).toContain('File not found');
  });
});
