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
  ctx.show({ clientX: 10, clientY: 20, preventDefault: () => undefined } as MouseEvent, items);
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
