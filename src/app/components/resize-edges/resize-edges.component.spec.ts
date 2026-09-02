import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { WindowService } from '../../services/window.service';
import { ResizeEdgesComponent } from './resize-edges.component';

function windowStub(overrides: Record<string, unknown> = {}) {
  return {
    customResize: true,
    maximized: vi.fn(() => false),
    fullscreen: vi.fn(() => false),
    startResizeDragging: vi.fn(async () => undefined),
    ...overrides,
  };
}

function setup(stub = windowStub()) {
  TestBed.configureTestingModule({
    imports: [ResizeEdgesComponent],
    providers: [{ provide: WindowService, useValue: stub }],
  });
  const fixture = TestBed.createComponent(ResizeEdgesComponent);
  fixture.detectChanges();
  return { stub, el: fixture.nativeElement as HTMLElement };
}

describe('ResizeEdgesComponent', () => {
  it('renders eight handles on a resizable frameless window', () => {
    const { el } = setup();
    expect(el.querySelectorAll('[data-resize]')).toHaveLength(8);
  });

  it('renders nothing when native resize is available', () => {
    const { el } = setup(windowStub({ customResize: false }));
    expect(el.querySelector('[data-resize]')).toBeNull();
  });

  it('renders nothing while maximized', () => {
    const { el } = setup(windowStub({ maximized: vi.fn(() => true) }));
    expect(el.querySelector('[data-resize]')).toBeNull();
  });

  it('renders nothing while fullscreen', () => {
    const { el } = setup(windowStub({ fullscreen: vi.fn(() => true) }));
    expect(el.querySelector('[data-resize]')).toBeNull();
  });

  it('a primary-button mousedown on a handle starts a resize in that direction', () => {
    const { el, stub } = setup();
    const handle = el.querySelector<HTMLElement>('[data-resize="SouthEast"]');
    handle?.dispatchEvent(new MouseEvent('mousedown', { button: 0, bubbles: true }));
    expect(stub.startResizeDragging).toHaveBeenCalledWith('SouthEast');
  });

  it('ignores secondary-button presses', () => {
    const { el, stub } = setup();
    const handle = el.querySelector<HTMLElement>('[data-resize="North"]');
    handle?.dispatchEvent(new MouseEvent('mousedown', { button: 2, bubbles: true }));
    expect(stub.startResizeDragging).not.toHaveBeenCalled();
  });
});
