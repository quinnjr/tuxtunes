import { TestBed } from '@angular/core/testing';
import { FaIconLibrary } from '@fortawesome/angular-fontawesome';
import { describe, expect, it, vi } from 'vitest';
import { IconRegistry } from './icon-registry';

describe('IconRegistry', () => {
  it('registers every transport + sidebar icon with the FontAwesome library', () => {
    const addIcons = vi.fn();
    TestBed.configureTestingModule({
      providers: [{ provide: FaIconLibrary, useValue: { addIcons } }, IconRegistry],
    });
    TestBed.inject(IconRegistry);
    expect(addIcons).toHaveBeenCalledOnce();
    // The constructor passes one icon per argument; assert the count.
    // (Specific icons are checked by template tests further down the
    // tree; we just verify the spread reaches the library.)
    expect(addIcons.mock.calls[0].length).toBe(12);
  });
});
