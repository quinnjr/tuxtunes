import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService } from '../../services/context-menu.service';
import { ContextMenuComponent } from './context-menu.component';

function setup() {
  TestBed.configureTestingModule({
    imports: [ContextMenuComponent],
    providers: [ContextMenuService],
  });
  const fixture = TestBed.createComponent(ContextMenuComponent);
  fixture.detectChanges();
  return {
    fixture,
    el: fixture.nativeElement as HTMLElement,
    ctx: TestBed.inject(ContextMenuService),
  };
}

function showMenu(
  ctx: ContextMenuService,
  items: { label: string; action?: () => void }[] = [{ label: 'Play' }],
) {
  ctx.show(
    {
      clientX: 10,
      clientY: 20,
      preventDefault: () => undefined,
      stopPropagation: () => undefined,
    } as unknown as MouseEvent,
    items,
  );
}

describe('ContextMenuComponent', () => {
  it('renders nothing when the service has no open menu', () => {
    const { el, ctx } = setup();
    expect(el.querySelector('ul')).toBeNull();
    expect(ctx.open()).toBeNull();
  });

  it('renders items when the service opens a menu', () => {
    const { fixture, el, ctx } = setup();
    showMenu(ctx, [{ label: 'Play' }, { label: '---' }, { label: 'Delete' }]);
    fixture.detectChanges();
    const items = el.querySelectorAll('button');
    expect(items.length).toBe(2);
    const sep = el.querySelector('[role="separator"]');
    expect(sep).not.toBeNull();
  });

  it('isDivider() detects the --- sentinel', () => {
    const { fixture } = setup();
    const cmp = fixture.componentInstance as unknown as {
      isDivider(item: { label: string }): boolean;
    };
    expect(cmp.isDivider({ label: '---' })).toBe(true);
    expect(cmp.isDivider({ label: 'Play' })).toBe(false);
  });

  it('clicking the backdrop dismisses the menu', () => {
    const { fixture, el, ctx } = setup();
    showMenu(ctx);
    fixture.detectChanges();
    const backdrop = el.querySelector('.fixed.inset-0') as HTMLElement | null;
    backdrop?.click();
    fixture.detectChanges();
    expect(ctx.open()).toBeNull();
  });

  it('ESC dismisses via the document keydown listener', () => {
    const { fixture, ctx } = setup();
    showMenu(ctx);
    fixture.detectChanges();
    const cmp = fixture.componentInstance as unknown as { onEscape(): void };
    cmp.onEscape();
    expect(ctx.open()).toBeNull();
  });

  it('document:contextmenu hides the menu when target is outside', () => {
    const { fixture, ctx } = setup();
    showMenu(ctx);
    fixture.detectChanges();
    const cmp = fixture.componentInstance as unknown as {
      onContext(event: MouseEvent): void;
    };
    const fakeOutside = { target: document.body } as unknown as MouseEvent;
    cmp.onContext(fakeOutside);
    expect(ctx.open()).toBeNull();
  });

  it('document:contextmenu inside the menu does NOT hide it', () => {
    const { fixture, ctx } = setup();
    showMenu(ctx);
    fixture.detectChanges();
    const inner = document.createElement('div');
    inner.dataset['contextMenu'] = '';
    const child = document.createElement('span');
    inner.append(child);
    const cmp = fixture.componentInstance as unknown as {
      onContext(event: MouseEvent): void;
    };
    cmp.onContext({ target: child } as unknown as MouseEvent);
    expect(ctx.open()).not.toBeNull();
  });

  it('document:contextmenu when no menu is open is a no-op', () => {
    const { fixture } = setup();
    const cmp = fixture.componentInstance as unknown as {
      onContext(event: MouseEvent): void;
    };
    expect(() => cmp.onContext({ target: document.body } as unknown as MouseEvent)).not.toThrow();
  });

  it('renders a checkmark only for checked items', () => {
    const { fixture, el, ctx } = setup();
    ctx.show(
      {
        clientX: 0,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [
        { label: 'Title', checked: true },
        { label: 'Plays', checked: false },
      ],
    );
    fixture.detectChanges();
    const buttons = [...el.querySelectorAll('button')];
    expect(buttons[0].textContent).toContain('✓');
    expect(buttons[1].textContent).not.toContain('✓');
  });

  it('an item with children shows a submenu indicator and opens the flyout on hover', () => {
    const { fixture, el, ctx } = setup();
    const child = vi.fn();
    ctx.show(
      {
        clientX: 0,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [{ label: 'Add to Playlist', children: [{ label: 'Mix', action: child }] }],
    );
    fixture.detectChanges();
    const parent = el.querySelector('button')!;
    expect(parent.textContent).toContain('▸');
    expect(el.querySelector('[data-submenu]')).toBeNull();
    parent.dispatchEvent(new MouseEvent('mouseenter'));
    fixture.detectChanges();
    const flyout = el.querySelector('[data-submenu]')!;
    expect(flyout).not.toBeNull();
    expect(flyout.textContent).toContain('Mix');
  });

  it('clicking a submenu child runs its action and dismisses the menu', () => {
    const { fixture, el, ctx } = setup();
    const child = vi.fn();
    ctx.show(
      {
        clientX: 0,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [{ label: 'Add to Playlist', children: [{ label: 'Mix', action: child }] }],
    );
    fixture.detectChanges();
    el.querySelector('button')!.dispatchEvent(new MouseEvent('mouseenter'));
    fixture.detectChanges();
    const childButton = el.querySelector<HTMLButtonElement>('[data-submenu] button')!;
    childButton.click();
    expect(child).toHaveBeenCalled();
    expect(ctx.open()).toBeNull();
  });

  it('hovering a plain item closes any open submenu', () => {
    const { fixture, el, ctx } = setup();
    ctx.show(
      {
        clientX: 0,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [{ label: 'Add to Playlist', children: [{ label: 'Mix' }] }, { label: 'Play' }],
    );
    fixture.detectChanges();
    const buttons = [...el.querySelectorAll('button')];
    buttons[0].dispatchEvent(new MouseEvent('mouseenter'));
    fixture.detectChanges();
    expect(el.querySelector('[data-submenu]')).not.toBeNull();
    buttons[1].dispatchEvent(new MouseEvent('mouseenter'));
    fixture.detectChanges();
    expect(el.querySelector('[data-submenu]')).toBeNull();
  });

  it('clicking a parent item with children does not dismiss the menu', () => {
    const { fixture, el, ctx } = setup();
    ctx.show(
      {
        clientX: 0,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [{ label: 'Add to Playlist', children: [{ label: 'Mix' }] }],
    );
    fixture.detectChanges();
    el.querySelector('button')!.click();
    fixture.detectChanges();
    expect(ctx.open()).not.toBeNull();
    expect(el.querySelector('[data-submenu]')).not.toBeNull();
  });

  it('a menu opened from a real bubbling contextmenu event stays open', () => {
    // Regression: show() must stop propagation, otherwise the very
    // event that opened the menu bubbles on to this component's
    // document-level contextmenu handler, which dismisses it in the
    // same dispatch — the menu never appears on screen.
    const { fixture, ctx } = setup();
    const outside = document.createElement('div');
    document.body.append(outside);
    outside.addEventListener('contextmenu', (e) => ctx.show(e as MouseEvent, [{ label: 'Play' }]));
    try {
      outside.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
      fixture.detectChanges();
      expect(ctx.open()).not.toBeNull();
    } finally {
      outside.remove();
    }
  });

  it('clicking an action item runs the action and dismisses the menu', () => {
    const { fixture, el, ctx } = setup();
    const action = vi.fn();
    showMenu(ctx, [{ label: 'Play', action }]);
    fixture.detectChanges();
    const button = el.querySelector('button')!;
    button.click();
    expect(action).toHaveBeenCalled();
  });
});
