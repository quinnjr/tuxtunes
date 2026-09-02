import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { WindowService } from '../../services/window.service';
import { WindowControlsComponent } from './window-controls.component';

interface Internals {
  close(): void;
  minimize(): void;
  zoom(event: MouseEvent): void;
}

function windowStub(overrides: Record<string, unknown> = {}) {
  return {
    customControls: true,
    customResize: true,
    maximized: vi.fn(() => false),
    fullscreen: vi.fn(() => false),
    close: vi.fn(async () => undefined),
    minimize: vi.fn(async () => undefined),
    toggleMaximize: vi.fn(async () => undefined),
    toggleFullscreen: vi.fn(async () => undefined),
    ...overrides,
  };
}

function setup(stub = windowStub()) {
  TestBed.configureTestingModule({
    imports: [WindowControlsComponent],
    providers: [{ provide: WindowService, useValue: stub }],
  });
  const fixture = TestBed.createComponent(WindowControlsComponent);
  fixture.detectChanges();
  return {
    fixture,
    stub,
    cmp: fixture.componentInstance as unknown as Internals,
    el: fixture.nativeElement as HTMLElement,
  };
}

describe('WindowControlsComponent', () => {
  it('renders three traffic lights when the platform draws its own controls', () => {
    const { el } = setup();
    expect(el.querySelectorAll('button.mac-traffic-light')).toHaveLength(3);
  });

  it('renders nothing when the OS provides native controls', () => {
    const { el } = setup(windowStub({ customControls: false }));
    expect(el.querySelector('button')).toBeNull();
  });

  it('close and minimize buttons call the window service', () => {
    const { el, stub } = setup();
    el.querySelector<HTMLButtonElement>('.mac-traffic-light-close')?.click();
    el.querySelector<HTMLButtonElement>('.mac-traffic-light-minimize')?.click();
    expect(stub.close).toHaveBeenCalledOnce();
    expect(stub.minimize).toHaveBeenCalledOnce();
  });

  it('zoom toggles maximize on plain click and fullscreen on alt-click', () => {
    const { cmp, stub } = setup();
    cmp.zoom(new MouseEvent('click'));
    expect(stub.toggleMaximize).toHaveBeenCalledOnce();
    expect(stub.toggleFullscreen).not.toHaveBeenCalled();
    cmp.zoom(new MouseEvent('click', { altKey: true }));
    expect(stub.toggleFullscreen).toHaveBeenCalledOnce();
  });

  it('labels the zoom button Restore while maximized', () => {
    const { el } = setup(windowStub({ maximized: vi.fn(() => true) }));
    const zoom = el.querySelector<HTMLButtonElement>('.mac-traffic-light-zoom');
    expect(zoom?.getAttribute('aria-label')).toBe('Restore');
  });
});
