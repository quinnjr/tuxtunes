import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { ConvertService } from '../../services/convert.service';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { SyncService } from '../../services/sync.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { StatusBarComponent } from './status-bar.component';

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [StatusBarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(StatusBarComponent);
  fixture.detectChanges();
  return {
    fixture,
    el: fixture.nativeElement as HTMLElement,
    convert: TestBed.inject(ConvertService),
    library: TestBed.inject(LibraryService),
    stub,
    sync: TestBed.inject(SyncService),
    ui: TestBed.inject(UiService),
  };
}

describe('StatusBarComponent', () => {
  it('renders the loading placeholder before stats arrive', () => {
    const { el } = setup();
    expect(el.textContent).toContain('Loading library');
  });

  it('renders songs / duration / size when stats are populated', () => {
    const { fixture, library, el } = setup();
    library.stats.set({
      trackCount: 1,
      totalDurationMs: 60_000,
      totalSizeBytes: 1024,
    });
    fixture.detectChanges();
    expect(el.textContent).toContain('1 song');
    expect(el.textContent).toContain('0:01:00');
    expect(el.textContent).toContain('1.00 KiB');
  });

  it('pluralizes "songs" past one', () => {
    const { fixture, library, el } = setup();
    library.stats.set({
      trackCount: 42,
      totalDurationMs: 0,
      totalSizeBytes: 0,
    });
    fixture.detectChanges();
    expect(el.textContent).toContain('42 songs');
  });

  it('shows the sync label only when SyncService is running or errored', () => {
    const { fixture, sync, el } = setup();
    expect(el.textContent ?? '').not.toContain('Syncing');
    sync.progress.set({
      sourceId: 1,
      phase: 'decoding',
      current: 0,
      total: 0,
      message: '',
    });
    fixture.detectChanges();
    expect(el.textContent).toContain('Syncing');

    sync.lastError.set({ sourceId: 1, error: 'x' });
    fixture.detectChanges();
    expect(el.textContent).toContain('Sync error');
  });

  it('shows convert progress with its percentage, outranking the sync label', () => {
    const { fixture, convert, sync, el } = setup();
    sync.progress.set({
      sourceId: 1,
      phase: 'decoding',
      current: 0,
      total: 0,
      message: '',
    });
    convert.progress.set({ current: 1, total: 4, trackId: 2, title: 'Song', percent: 63 });
    fixture.detectChanges();
    expect(el.textContent).toContain('Converting 2 of 4 · 63%');
    expect(el.textContent).not.toContain('Syncing');
  });

  it('omits the percentage when the track duration is unknown', () => {
    const { fixture, convert, el } = setup();
    convert.progress.set({ current: 0, total: 1, trackId: 2, title: 'Song', percent: null });
    fixture.detectChanges();
    expect(el.textContent).toContain('Converting 1 of 1');
    expect(el.textContent).not.toContain('%');
  });

  it('offers a cancel button only while a conversion is running', () => {
    const { fixture, convert, el, stub } = setup();
    expect(el.querySelector('button')).toBeNull();

    convert.progress.set({ current: 0, total: 3, trackId: 1, title: 'Song', percent: 10 });
    fixture.detectChanges();
    const button = el.querySelector('button');
    expect(button?.textContent).toContain('Cancel');

    button?.click();
    expect(stub.invoke).toHaveBeenCalledWith('cancel_convert');

    convert.lastComplete.set({
      total: 3,
      converted: 1,
      failed: 0,
      addedToLibrary: 0,
      cancelled: true,
      format: 'flac',
    });
    fixture.detectChanges();
    expect(el.querySelector('button')).toBeNull();
  });

  it('shows the last playback error as an alert, taking precedence over the sync label', () => {
    const { fixture, el, ui, sync } = setup();
    sync.progress.set({
      source_id: 1,
      phase: 'ApplyingTracks',
      current: 1,
      total: 2,
      message: '',
    } as never);
    fixture.detectChanges();
    expect(el.textContent).toContain('Syncing');
    ui.lastError.set('File not found: /x.mp3');
    fixture.detectChanges();
    const alert = el.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain('File not found: /x.mp3');
    expect(el.textContent).not.toContain('Syncing');
    ui.lastError.set(null);
    fixture.detectChanges();
    expect(el.querySelector('[role="alert"]')).toBeNull();
  });
});
