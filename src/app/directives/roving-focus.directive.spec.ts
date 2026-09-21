import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { ROVING_FOCUS_ORIENTATION, RovingFocusDirective } from './roving-focus.directive';

@Component({
  imports: [RovingFocusDirective],
  providers: [{ provide: ROVING_FOCUS_ORIENTATION, useValue: 'horizontal' }],
  template: `
    <div
      role="radiogroup"
      aria-label="Modes"
      appRovingFocus
      (appRovingFocusSelect)="select($event)"
    >
      @for (m of modes; track m) {
        <button type="button" role="radio" [attr.aria-checked]="selected === $index">
          {{ m }}
        </button>
      }
    </div>
  `,
})
class HeaterHost {
  modes = ['tracks', 'albums', 'artists'];
  selected = 0;

  select(index: number): void {
    this.selected = index;
  }
}

@Component({
  imports: [RovingFocusDirective],
  providers: [{ provide: ROVING_FOCUS_ORIENTATION, useValue: 'vertical' }],
  template: `
    <ul role="menu" appRovingFocus (appRovingFocusSelect)="select($event)">
      <li role="none"><button type="button" role="menuitem">One</button></li>
      <li role="none"><button type="button" role="menuitem">Two</button></li>
      <li role="none"><button type="button" role="menuitem">Three</button></li>
    </ul>
  `,
})
class MenuHost {
  picked = -1;

  select(index: number): void {
    this.picked = index;
  }
}

describe('RovingFocusDirective', () => {
  const setup = async <T>(host: new () => T) => {
    await TestBed.configureTestingModule({ imports: [host] }).compileComponents();
    const fixture = TestBed.createComponent(host);
    document.body.append(fixture.nativeElement as HTMLElement);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    return { fixture, el };
  };

  afterEach(() => {
    document.body.replaceChildren();
  });

  const items = (el: HTMLElement) => [
    ...el.querySelectorAll<HTMLElement>('[role="radio"], [role="menuitem"]'),
  ];

  const press = (target: HTMLElement, key: string) => {
    const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
    target.dispatchEvent(event);
    return event;
  };

  it('gives the group a single tab stop at rest, on the checked item', async () => {
    const { el } = await setup(HeaterHost);
    const all = items(el);
    // Exactly one tab stop before any interaction.
    const stops = all.filter((i) => i.getAttribute('tabindex') !== '-1');
    expect(stops).toHaveLength(1);
    // Every other item is explicitly -1, not left without an attribute.
    expect(all.every((i) => i.getAttribute('tabindex') !== null)).toBe(true);
    // HeaterHost starts with item 0 checked.
    expect(stops[0]).toBe(all[0]);
  });

  it('moves the single tab stop to the newly focused item', async () => {
    const { el } = await setup(HeaterHost);
    const [first, second] = items(el);
    first.focus();
    press(first, 'ArrowRight');
    const all = items(el);
    const stops = all.filter((i) => i.getAttribute('tabindex') !== '-1');
    expect(stops).toHaveLength(1);
    expect(stops[0]).toBe(second);
  });

  it('moves focus and selection right with ArrowRight, wrapping at the end', async () => {
    const { fixture, el } = await setup(HeaterHost);
    const [first, second, third] = items(el);
    first.focus();

    press(first, 'ArrowRight');
    expect(document.activeElement).toBe(second);
    expect(fixture.componentInstance.selected).toBe(1);

    press(second, 'ArrowRight');
    expect(document.activeElement).toBe(third);

    press(third, 'ArrowRight');
    expect(document.activeElement).toBe(first);
    expect(fixture.componentInstance.selected).toBe(0);
  });

  it('moves left with ArrowLeft and Home/End jump to the ends', async () => {
    const { fixture, el } = await setup(HeaterHost);
    const [first, , third] = items(el);
    first.focus();

    press(first, 'ArrowLeft');
    expect(document.activeElement).toBe(third);
    expect(fixture.componentInstance.selected).toBe(2);

    press(third, 'End');
    expect(document.activeElement).toBe(third);

    press(third, 'Home');
    expect(document.activeElement).toBe(first);
  });

  it('ignores the cross-axis arrows for a horizontal group', async () => {
    const { el } = await setup(HeaterHost);
    const [first] = items(el);
    first.focus();
    const event = press(first, 'ArrowDown');
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(first);
  });

  it('moves the roving tab stop to the newly focused item', async () => {
    const { el } = await setup(HeaterHost);
    const [first, second] = items(el);
    first.focus();
    press(first, 'ArrowRight');
    expect(second.getAttribute('tabindex')).toBe('0');
    expect(first.getAttribute('tabindex')).toBe('-1');
  });

  it('walks a vertical menu and ignores horizontal arrows', async () => {
    const { fixture, el } = await setup(MenuHost);
    const [first, second] = items(el);
    first.focus();

    const right = press(first, 'ArrowRight');
    expect(right.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(first);

    press(first, 'ArrowDown');
    expect(document.activeElement).toBe(second);
    expect(fixture.componentInstance.picked).toBe(1);
  });

  it('is a no-op when there are no items yet', async () => {
    await TestBed.configureTestingModule({ imports: [EmptyHost] }).compileComponents();
    const fixture = TestBed.createComponent(EmptyHost);
    document.body.append(fixture.nativeElement as HTMLElement);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    const group = el.querySelector<HTMLElement>('[role="radiogroup"]')!;

    const event = new KeyboardEvent('keydown', {
      key: 'ArrowRight',
      bubbles: true,
      cancelable: true,
    });
    group.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  it('skips disabled items and never lands the tab stop on one', async () => {
    await TestBed.configureTestingModule({ imports: [DisabledHost] }).compileComponents();
    const fixture = TestBed.createComponent(DisabledHost);
    document.body.append(fixture.nativeElement as HTMLElement);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    const all = items(el);
    expect(all).toHaveLength(3);

    // The middle item is aria-disabled; the tab stop must be on item 0.
    const stops = all.filter((i) => i.getAttribute('tabindex') !== '-1');
    expect(stops).toHaveLength(1);
    expect(stops[0]).toBe(all[0]);

    // ArrowRight from item 0 skips the disabled item to item 2.
    all[0].focus();
    press(all[0], 'ArrowRight');
    expect(document.activeElement).toBe(all[2]);

    // And back, skipping it again.
    press(all[2], 'ArrowRight');
    expect(document.activeElement).toBe(all[0]);
  });
});

@Component({
  imports: [RovingFocusDirective],
  providers: [{ provide: ROVING_FOCUS_ORIENTATION, useValue: 'horizontal' }],
  template: `
    <div role="radiogroup" appRovingFocus>
      <button type="button" role="radio" [attr.aria-checked]="true">One</button>
      <button type="button" role="radio" aria-disabled="true" [attr.aria-checked]="false">
        Two
      </button>
      <button type="button" role="radio" [attr.aria-checked]="false">Three</button>
    </div>
  `,
})
class DisabledHost {}

@Component({
  imports: [RovingFocusDirective],
  template: `
    <div role="radiogroup" appRovingFocus>
      @for (m of modes; track m) {
        <button type="button" role="radio" [attr.aria-checked]="false">{{ m }}</button>
      }
    </div>
  `,
})
class EmptyHost {
  modes: string[] = [];
}
