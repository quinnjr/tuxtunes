import { signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { WindowService } from '../../services/window.service';
import { WindowControlsComponent } from './window-controls.component';

interface Internals {
  zoom(event: MouseEvent): void;
}

/** Live signals so a test can flip window state on one mounted fixture. */
function windowStub(customControls = true) {
  return {
    customControls: signal(customControls),
    maximized: signal(false),
    fullscreen: signal(false),
    close: vi.fn(async () => undefined),
    minimize: vi.fn(async () => undefined),
    toggleMaximize: vi.fn(async () => undefined),
    toggleFullscreen: vi.fn(async () => undefined),
  };
}

function setup(stub = windowStub()) {
  TestBed.configureTestingModule({
    imports: [WindowControlsComponent],
    providers: [{ provide: WindowService, useValue: stub }],
  });
  const fixture = TestBed.createComponent(WindowControlsComponent);
  fixture.detectChanges();
  const el = fixture.nativeElement as HTMLElement;
  return {
    fixture,
    stub,
    el,
    cmp: fixture.componentInstance as unknown as Internals,
    zoomButton: () => el.querySelector<HTMLButtonElement>('.win-control-maximize'),
  };
}

describe('WindowControlsComponent', () => {
  it('renders minimize, maximize, close in that order, all out of the tab ring', () => {
    const { el } = setup();
    const buttons = [...el.querySelectorAll<HTMLButtonElement>('button.win-control')];
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual([
      'Minimize',
      'Maximize (F11 or Alt-click: full screen)',
      'Close',
    ]);
    expect(buttons.every((b) => b.tabIndex === -1)).toBe(true);
  });

  it('renders nothing when the OS provides native controls', () => {
    const { el } = setup(windowStub(false));
    expect(el.querySelector('button')).toBeNull();
  });

  it('close and minimize buttons call the window service', () => {
    const { el, stub } = setup();
    el.querySelector<HTMLButtonElement>('.win-control-close')?.click();
    el.querySelector<HTMLButtonElement>('.win-control-minimize')?.click();
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

  it('a plain click while fullscreen exits fullscreen instead of toggling maximize', () => {
    const { cmp, stub, fixture } = setup();
    stub.fullscreen.set(true);
    fixture.detectChanges();
    cmp.zoom(new MouseEvent('click'));
    expect(stub.toggleFullscreen).toHaveBeenCalledOnce();
    expect(stub.toggleMaximize).not.toHaveBeenCalled();
  });

  it('re-labels the zoom button as the window state changes', () => {
    const { stub, fixture, zoomButton } = setup();
    expect(zoomButton()?.dataset['zoom']).toBe('maximize');
    expect(zoomButton()?.getAttribute('aria-label')).toBe(
      'Maximize (F11 or Alt-click: full screen)',
    );

    stub.maximized.set(true);
    fixture.detectChanges();
    expect(zoomButton()?.dataset['zoom']).toBe('restore');
    expect(zoomButton()?.getAttribute('aria-label')).toBe('Restore');
    expect(zoomButton()?.title).toBe('Restore');

    stub.fullscreen.set(true);
    fixture.detectChanges();
    expect(zoomButton()?.dataset['zoom']).toBe('exit-fullscreen');
    expect(zoomButton()?.getAttribute('aria-label')).toBe('Exit full screen');
  });
});
