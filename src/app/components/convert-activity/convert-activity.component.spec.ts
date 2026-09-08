import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { ConvertService } from '../../services/convert.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { ConvertActivityComponent } from './convert-activity.component';

const progress = (current: number, total: number, percent: number | null) => ({
  current,
  total,
  title: 'Song',
  percent,
});

async function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [ConvertActivityComponent],
    providers: appProviders(stub),
  });
  const convert = TestBed.inject(ConvertService);
  // Let the service's listen() calls register before events are emitted.
  await Promise.resolve();
  const fixture = TestBed.createComponent(ConvertActivityComponent);
  fixture.detectChanges();
  return { fixture, el: fixture.nativeElement as HTMLElement, convert, stub };
}

describe('ConvertActivityComponent', () => {
  it('renders nothing while idle', async () => {
    const { el } = await setup();
    expect(el.textContent?.trim()).toBe('');
  });

  it('shows a bare label between queueing and the first progress event', async () => {
    const { fixture, el, convert } = await setup();
    void convert.convert([1], 'flac');
    fixture.detectChanges();
    expect(el.textContent).toContain('Converting…');
  });

  it('shows position and percentage, omitting the title by default', async () => {
    const { fixture, el, stub } = await setup();
    stub.emit('fs:convert-progress', progress(1, 4, 63));
    fixture.detectChanges();
    expect(el.textContent).toContain('Converting 2 of 4 · 63%');
    expect(el.textContent).not.toContain('Song');
  });

  it('includes the title when asked and drops the percentage when unknown', async () => {
    const { fixture, el, stub } = await setup();
    fixture.componentRef.setInput('showTitle', true);
    stub.emit('fs:convert-progress', progress(0, 1, null));
    fixture.detectChanges();
    expect(el.textContent).toContain('Converting 1 of 1: Song');
    expect(el.textContent).not.toContain('%');
  });

  it('cancel reaches the backend and the button leaves once the batch completes', async () => {
    const { fixture, el, stub } = await setup();
    stub.emit('fs:convert-progress', progress(0, 3, 10));
    fixture.detectChanges();
    const button = el.querySelector('button');
    expect(button?.textContent).toContain('Cancel');

    button?.click();
    expect(stub.invoke).toHaveBeenCalledWith('cancel_convert');

    stub.emit('fs:convert-complete', {
      total: 3,
      converted: 1,
      failed: 0,
      added_to_library: 0,
      cancelled: true,
      format: 'flac',
    });
    fixture.detectChanges();
    expect(el.querySelector('button')).toBeNull();
  });
});
