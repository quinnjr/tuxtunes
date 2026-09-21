import { By } from '@angular/platform-browser';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { ModalSheetDirective } from './modal-sheet.directive';

@Component({
  imports: [ModalSheetDirective],
  template: `
    <button type="button" data-testid="opener">Open</button>
    @if (open) {
      <div appModalSheet aria-labelledby="t" (appModalSheetEscape)="onEscape()">
        <h2 id="t">Title</h2>
        <button type="button" data-testid="first">First</button>
        <button type="button" data-testid="last">Last</button>
      </div>
    }
  `,
})
class Host {
  open = false;
  escapes = 0;

  onEscape(): void {
    this.escapes += 1;
  }
}

describe('ModalSheetDirective', () => {
  const setup = async () => {
    await TestBed.configureTestingModule({ imports: [Host] }).compileComponents();
    const fixture = TestBed.createComponent(Host);
    // Append to the document so `.focus()` actually moves focus; an
    // unattached node cannot receive it and `activeElement` stays on body.
    document.body.append(fixture.nativeElement as HTMLElement);
    const el = fixture.nativeElement as HTMLElement;
    const opener = el.querySelector<HTMLButtonElement>('[data-testid="opener"]')!;
    opener.focus();
    fixture.componentInstance.open = true;
    fixture.detectChanges();
    return {
      fixture,
      opener,
      sheet: el.querySelector<HTMLElement>('[appModalSheet]')!,
      first: el.querySelector<HTMLButtonElement>('[data-testid="first"]')!,
      last: el.querySelector<HTMLButtonElement>('[data-testid="last"]')!,
    };
  };

  afterEach(() => {
    document.body.replaceChildren();
  });

  it('marks its host as a modal dialog', async () => {
    const { sheet } = await setup();
    expect(sheet.getAttribute('role')).toBe('dialog');
    expect(sheet.getAttribute('aria-modal')).toBe('true');
    expect(sheet.getAttribute('aria-labelledby')).toBe('t');
  });

  it('omits aria-modal when the sheet is non-modal (a popover)', async () => {
    @Component({
      imports: [ModalSheetDirective],
      template: `<div appModalSheet [appModalSheetModal]="false"><button>One</button></div>`,
    })
    class Popover {}

    await TestBed.configureTestingModule({ imports: [Popover] }).compileComponents();
    const fixture = TestBed.createComponent(Popover);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    const host = el.querySelector('[appmodalsheet]')!;
    // Still a dialog, but not advertised as modal.
    expect(host.getAttribute('role')).toBe('dialog');
    expect(host.getAttribute('aria-modal')).toBeNull();
  });

  it('moves focus into the sheet on init', async () => {
    const { first } = await setup();
    expect(document.activeElement).toBe(first);
  });

  it('emits appModalSheetEscape on Escape', async () => {
    const { fixture, sheet } = await setup();
    sheet.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(fixture.componentInstance.escapes).toBe(1);
  });

  it('wraps Tab from the last element back to the first', async () => {
    const { sheet, first, last } = await setup();
    last.focus();
    const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
    sheet.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
  });

  it('wraps Shift+Tab from the first element back to the last', async () => {
    const { sheet, first, last } = await setup();
    first.focus();
    const event = new KeyboardEvent('keydown', {
      key: 'Tab',
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    sheet.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
  });

  it('captures the element focused before it opens, then returns focus on destroy', async () => {
    const { fixture, opener } = await setup();
    const directive = fixture.debugElement
      .query(By.directive(ModalSheetDirective))
      .injector.get(ModalSheetDirective);
    expect(directive.previouslyFocused).toBe(opener);

    // Simulate real teardown: removing the subtree drops focus to <body>,
    // which is the condition under which the restore runs.
    (document.activeElement as HTMLElement | null)?.blur?.();
    const focus = vi.spyOn(opener, 'focus');
    directive.ngOnDestroy();
    await Promise.resolve();
    expect(focus).toHaveBeenCalled();
  });

  it('does NOT steal focus back when another surface already claimed it', async () => {
    const { fixture, opener } = await setup();
    const directive = fixture.debugElement
      .query(By.directive(ModalSheetDirective))
      .injector.get(ModalSheetDirective);

    // Another modal opened in the same tick and moved focus into itself.
    const elsewhere = document.createElement('button');
    document.body.append(elsewhere);
    elsewhere.focus();
    try {
      const focus = vi.spyOn(opener, 'focus');
      directive.ngOnDestroy();
      await Promise.resolve();
      expect(focus).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(elsewhere);
    } finally {
      elsewhere.remove();
    }
  });

  it('pulls focus back inside when it is on a non-listed node (the panel)', async () => {
    const { sheet, first, last } = await setup();
    // Focus the panel itself (tabindex="-1"), as a padding click would.
    sheet.focus();
    expect(document.activeElement).toBe(sheet);

    sheet.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
    expect(document.activeElement).toBe(first);

    sheet.focus();
    sheet.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true }),
    );
    expect(document.activeElement).toBe(last);
  });
});
