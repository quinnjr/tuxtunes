import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { LibraryService } from '../../services/library.service';
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
    library: TestBed.inject(LibraryService),
    sync: TestBed.inject(SyncService),
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
});
