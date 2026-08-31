import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { LibraryService } from '../../services/library.service';
import { type TrackRow } from '../../services/playback.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { TrackInfoComponent } from './track-info.component';

const ROW: TrackRow = {
  id: 7,
  title: 'Anthem, Pt. 2',
  artist: 'blink-182',
  album: 'TOYPAJ',
  albumArtist: null,
  genre: 'Punk',
  year: 2001,
  trackNumber: 1,
  discNumber: null,
  durationMs: 227_200,
  filePath: '/tmp/a.flac',
  sampleRate: 44_100,
  bitDepth: 16,
  kind: 'flac',
  playCount: 3,
  skipCount: 0,
  missing: false,
  artworkPath: null,
};

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [TrackInfoComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(TrackInfoComponent);
  const library = TestBed.inject(LibraryService);
  library.tracks.set([ROW]);
  fixture.detectChanges();
  return {
    fixture,
    el: fixture.nativeElement as HTMLElement,
    ui: TestBed.inject(UiService),
    library,
    stub,
  };
}

describe('TrackInfoComponent', () => {
  it('renders nothing while closed', () => {
    const { el } = setup();
    expect(el.querySelector('form')).toBeNull();
  });

  it('prefills every field from the track row when opened', () => {
    const { fixture, el, ui } = setup();
    ui.trackInfo.set({ trackId: 7 });
    fixture.detectChanges();
    const value = (name: string) =>
      el.querySelector<HTMLInputElement>(`input[name="${name}"]`)!.value;
    expect(value('title')).toBe('Anthem, Pt. 2');
    expect(value('artist')).toBe('blink-182');
    expect(value('album')).toBe('TOYPAJ');
    expect(value('albumArtist')).toBe('');
    expect(value('genre')).toBe('Punk');
    expect(value('year')).toBe('2001');
    expect(value('trackNumber')).toBe('1');
    expect(value('discNumber')).toBe('');
  });

  it('saving submits the edited patch and closes', async () => {
    const { fixture, el, ui, stub } = setup();
    ui.trackInfo.set({ trackId: 7 });
    fixture.detectChanges();
    const input = el.querySelector<HTMLInputElement>('input[name="artist"]')!;
    input.value = 'Blink One Eighty Two';
    input.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_track_metadata', {
      trackId: 7,
      edit: expect.objectContaining({
        title: 'Anthem, Pt. 2',
        artist: 'Blink One Eighty Two',
        genre: 'Punk',
        year: 2001,
        trackNumber: 1,
        discNumber: null,
      }) as Record<string, unknown>,
    });
    expect(ui.trackInfo()).toBeNull();
  });

  it('blank optional fields submit as null; blank title blocks the save', async () => {
    const { fixture, el, ui, stub } = setup();
    ui.trackInfo.set({ trackId: 7 });
    fixture.detectChanges();
    const genre = el.querySelector<HTMLInputElement>('input[name="genre"]')!;
    genre.value = '   ';
    genre.dispatchEvent(new Event('input'));
    const title = el.querySelector<HTMLInputElement>('input[name="title"]')!;
    title.value = '  ';
    title.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
    await fixture.whenStable();
    expect(stub.invoke).not.toHaveBeenCalledWith('update_track_metadata', expect.anything());
    expect(ui.trackInfo()).not.toBeNull();

    title.value = 'Kept';
    title.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_track_metadata', {
      trackId: 7,
      edit: expect.objectContaining({ title: 'Kept', genre: null }) as Record<string, unknown>,
    });
  });

  it('cancel closes without saving', () => {
    const { fixture, el, ui, stub } = setup();
    ui.trackInfo.set({ trackId: 7 });
    fixture.detectChanges();
    el.querySelector<HTMLButtonElement>('button[type="button"]')!.click();
    fixture.detectChanges();
    expect(stub.invoke).not.toHaveBeenCalledWith('update_track_metadata', expect.anything());
    expect(ui.trackInfo()).toBeNull();
  });

  it('a save failure reports the error and keeps the dialog open', async () => {
    const { fixture, el, ui } = setup();
    const library = TestBed.inject(LibraryService);
    vi.spyOn(library, 'updateTrackMetadata').mockRejectedValue(new Error('tag write failed'));
    ui.trackInfo.set({ trackId: 7 });
    fixture.detectChanges();
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
    await fixture.whenStable();
    await new Promise((r) => setTimeout(r));
    expect(ui.lastError()).toContain('tag write failed');
    expect(ui.trackInfo()).not.toBeNull();
  });
});
