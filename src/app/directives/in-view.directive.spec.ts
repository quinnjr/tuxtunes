import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { InViewDirective } from './in-view.directive';

@Component({
  imports: [InViewDirective],
  template: '<div (appInView)="seen = seen + 1"></div>',
})
class HostComponent {
  seen = 0;
}

type IOCallback = (entries: { isIntersecting: boolean }[]) => void;

function installObserverMock() {
  const callbacks: IOCallback[] = [];
  const observe = vi.fn();
  const disconnect = vi.fn();
  class MockIO {
    constructor(cb: IOCallback) {
      callbacks.push(cb);
    }
    observe = observe;
    disconnect = disconnect;
  }
  vi.stubGlobal('IntersectionObserver', MockIO);
  return { callbacks, observe, disconnect };
}

describe('InViewDirective', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('emits immediately when IntersectionObserver is unavailable', () => {
    vi.stubGlobal('IntersectionObserver', undefined);
    TestBed.configureTestingModule({ imports: [HostComponent] });
    const fixture = TestBed.createComponent(HostComponent);
    fixture.detectChanges();
    expect(fixture.componentInstance.seen).toBe(1);
  });

  it('emits once on first intersection and disconnects', () => {
    const io = installObserverMock();
    TestBed.configureTestingModule({ imports: [HostComponent] });
    const fixture = TestBed.createComponent(HostComponent);
    fixture.detectChanges();
    expect(io.observe).toHaveBeenCalledTimes(1);
    expect(fixture.componentInstance.seen).toBe(0);
    io.callbacks[0]([{ isIntersecting: false }]);
    expect(fixture.componentInstance.seen).toBe(0);
    io.callbacks[0]([{ isIntersecting: true }]);
    io.callbacks[0]([{ isIntersecting: true }]);
    expect(fixture.componentInstance.seen).toBe(1);
    expect(io.disconnect).toHaveBeenCalled();
  });
});
