import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { App } from './app';
import { LibraryService } from './services/library.service';
import { appProviders, tauriStub } from './test-helpers';

describe('App', () => {
  it('refreshes library stats on init', () => {
    const stub = tauriStub();
    TestBed.configureTestingModule({
      imports: [App],
      providers: appProviders(stub),
    });
    const fixture = TestBed.createComponent(App);
    const library = TestBed.inject(LibraryService);
    const spy = vi.spyOn(library, 'refreshStats').mockResolvedValue();
    fixture.detectChanges();
    expect(spy).toHaveBeenCalled();
  });
});
