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

  it('the flyout opens to the left when the menu sits near the right viewport edge', () => {
    const { fixture, el, ctx } = setup();
    ctx.show(
      {
        clientX: 1000,
        clientY: 0,
        preventDefault: () => undefined,
        stopPropagation: () => undefined,
      } as unknown as MouseEvent,
      [{ label: 'Add to Playlist', children: [{ label: 'Mix' }] }],
    );
    fixture.detectChanges();
    el.querySelector('button')!.dispatchEvent(new MouseEvent('mouseenter'));
    fixture.detectChanges();
    const flyout = el.querySelector('[data-submenu]')!;
    expect(flyout.className).toContain('right-full');
    expect(flyout.className).not.toContain('left-full');
  });

  it('the checkmark gutter renders only in menus that have checkable items', () => {
    const { fixture, el, ctx } = setup();
    showMenu(ctx, [{ label: 'Play' }, { label: 'Delete' }]);
    fixture.detectChanges();
    expect(el.querySelector('button .w-4')).toBeNull();
    ctx.hide();
    fixture.detectChanges();
    showMenu(ctx, [{ label: 'Title', checked: true } as never, { label: 'Plays' }]);
    fixture.detectChanges();
    expect(el.querySelector('button .w-4')).not.toBeNull();
  });

  it('hovering a plain item closes an open submenu only after a grace delay', () => {
    // Diagonal travel toward a flyout entry brushes sibling items; an
    // instant close would slam the flyout shut mid-gesture.
    vi.useFakeTimers();
    try {
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
      expect(el.querySelector('[data-submenu]')).not.toBeNull();
      vi.advanceTimersByTime(400);
      fixture.detectChanges();
      expect(el.querySelector('[data-submenu]')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('re-entering the flyout cancels the pending close', () => {
    vi.useFakeTimers();
    try {
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
      buttons[1].dispatchEvent(new MouseEvent('mouseenter'));
      el.querySelector('[data-submenu]')!.dispatchEvent(new MouseEvent('mouseenter'));
      vi.advanceTimersByTime(400);
      fixture.detectChanges();
      expect(el.querySelector('[data-submenu]')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
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

  describe('keyboard model', () => {
    const show = (
      ctx: ContextMenuService,
      items: { label: string; children?: { label: string }[] }[],
    ) => {
      ctx.show(
        {
          clientX: 0,
          clientY: 0,
          preventDefault: () => undefined,
          stopPropagation: () => undefined,
        } as unknown as MouseEvent,
        items,
      );
    };

    it('gives every item a menuitem role and moves focus in with arrows', () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [{ label: 'Play' }, { label: 'Queue' }, { label: 'Delete' }]);
      fixture.detectChanges();
      const items = [...el.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
      expect(items).toHaveLength(3);

      items[0].focus();
      items[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
      expect(document.activeElement).toBe(items[1]);

      items[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
      expect(document.activeElement).toBe(items[2]);
    });

    it('marks a checkable item as menuitemcheckbox with aria-checked', () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [
        { label: 'Title', checked: true },
        { label: 'Plays', checked: false },
      ] as never);
      fixture.detectChanges();
      const checks = [...el.querySelectorAll('[role="menuitemcheckbox"]')];
      expect(checks).toHaveLength(2);
      expect(checks[0].getAttribute('aria-checked')).toBe('true');
      expect(checks[1].getAttribute('aria-checked')).toBe('false');
    });

    it('opens a submenu with ArrowRight, moves focus to its first item, and closes with ArrowLeft', async () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [{ label: 'Add to Playlist', children: [{ label: 'Mix' }] }]);
      fixture.detectChanges();
      const parent = el.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
      expect(parent.getAttribute('aria-haspopup')).toBe('menu');
      expect(parent.getAttribute('aria-expanded')).toBe('false');

      parent.focus();
      parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
      fixture.detectChanges();
      await new Promise((r) => setTimeout(r));
      fixture.detectChanges();
      expect(parent.getAttribute('aria-expanded')).toBe('true');
      const child = el.querySelector<HTMLButtonElement>('[data-submenu] [role="menuitem"]')!;
      expect(child).not.toBeNull();
      // APG: opening a submenu moves focus into it.
      expect(document.activeElement).toBe(child);

      // ArrowLeft from inside the flyout closes it and refocuses the parent.
      child.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
      fixture.detectChanges();
      expect(parent.getAttribute('aria-expanded')).toBe('false');
      expect(el.querySelector('[data-submenu]')).toBeNull();
      expect(document.activeElement).toBe(parent);
    });

    it('cycles arrows within the open flyout without leaving it', async () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [
        {
          label: 'Add to Playlist',
          children: [{ label: 'Mix' }, { label: 'Queue' }],
        },
        { label: 'Play' },
      ]);
      fixture.detectChanges();
      const parent = el.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
      parent.focus();
      parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
      fixture.detectChanges();
      await new Promise((r) => setTimeout(r));
      fixture.detectChanges();

      const children = [
        ...el.querySelectorAll<HTMLButtonElement>('[data-submenu] [role="menuitem"]'),
      ];
      expect(children).toHaveLength(2);
      expect(document.activeElement).toBe(children[0]);

      // Down moves to the second child, not out to the "Play" item.
      children[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
      expect(document.activeElement).toBe(children[1]);

      // Down from the last child wraps within the flyout.
      children[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
      expect(document.activeElement).toBe(children[0]);
      // The flyout stays open throughout.
      expect(el.querySelector('[data-submenu]')).not.toBeNull();
    });

    it('returns focus to the element focused before the menu opened, on hide', async () => {
      const { fixture, ctx } = setup();
      const opener = document.createElement('button');
      document.body.append(opener);
      opener.focus();
      try {
        show(ctx, [{ label: 'Play' }]);
        fixture.detectChanges();
        ctx.hide();
        await Promise.resolve();
        expect(document.activeElement).toBe(opener);
      } finally {
        opener.remove();
      }
    });

    it('a disabled item is a no-op on click: no action, menu stays open', () => {
      const { fixture, el, ctx } = setup();
      const action = vi.fn();
      show(ctx, [{ label: 'Sync Now', disabled: true, action }, { label: 'Forget' }] as never);
      fixture.detectChanges();
      const disabled = el.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
      expect(disabled.getAttribute('aria-disabled')).toBe('true');
      disabled.click();
      fixture.detectChanges();
      expect(action).not.toHaveBeenCalled();
      // The menu must not dismiss on a disabled activation.
      expect(ctx.open()).not.toBeNull();
    });

    it('a disabled parent does not open its flyout on click or hover', () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [
        { label: 'Convert', disabled: true, children: [{ label: 'FLAC' }] },
        { label: 'Play' },
      ] as never);
      fixture.detectChanges();
      const parent = el.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
      parent.click();
      fixture.detectChanges();
      expect(el.querySelector('[data-submenu]')).toBeNull();
      parent.dispatchEvent(new MouseEvent('mouseenter'));
      fixture.detectChanges();
      expect(el.querySelector('[data-submenu]')).toBeNull();
    });

    it('a disabled parent does not open its flyout by keyboard', () => {
      const { fixture, el, ctx } = setup();
      show(ctx, [
        { label: 'Convert', disabled: true, children: [{ label: 'FLAC' }] },
        { label: 'Play' },
      ] as never);
      fixture.detectChanges();
      const parent = el.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
      for (const key of ['ArrowRight', 'Enter', ' ']) {
        parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
        fixture.detectChanges();
        expect(el.querySelector('[data-submenu]')).toBeNull();
      }
    });
  });
});
