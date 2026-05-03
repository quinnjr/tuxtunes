import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService } from './context-menu.service';

function fakeMouseEvent(x: number, y: number): MouseEvent {
  return {
    clientX: x,
    clientY: y,
    preventDefault: vi.fn(),
  } as unknown as MouseEvent;
}

describe('ContextMenuService', () => {
  it('starts with no open menu', () => {
    const svc = new ContextMenuService();
    expect(svc.open()).toBeNull();
  });

  it('show() captures position and items, and calls preventDefault', () => {
    const svc = new ContextMenuService();
    const evt = fakeMouseEvent(120, 80);
    svc.show(evt, [{ label: 'Play' }]);
    expect(evt.preventDefault).toHaveBeenCalled();
    const state = svc.open();
    expect(state).not.toBeNull();
    expect(state?.x).toBe(120);
    expect(state?.y).toBe(80);
    expect(state?.items).toEqual([{ label: 'Play' }]);
  });

  it('hide() returns to null', () => {
    const svc = new ContextMenuService();
    svc.show(fakeMouseEvent(0, 0), [{ label: 'X' }]);
    svc.hide();
    expect(svc.open()).toBeNull();
  });

  it('run() hides the menu and invokes the item action', async () => {
    const svc = new ContextMenuService();
    svc.show(fakeMouseEvent(0, 0), [{ label: 'X' }]);
    const action = vi.fn();
    await svc.run({ label: 'Do', action });
    expect(action).toHaveBeenCalledTimes(1);
    expect(svc.open()).toBeNull();
  });

  it('run() awaits async actions', async () => {
    const svc = new ContextMenuService();
    let resolved = false;
    await svc.run({
      label: 'Async',
      action: async () => {
        await new Promise((r) => setTimeout(r, 1));
        resolved = true;
      },
    });
    expect(resolved).toBe(true);
  });

  it('run() skips the action when disabled', async () => {
    const svc = new ContextMenuService();
    const action = vi.fn();
    await svc.run({ label: 'Nope', action, disabled: true });
    expect(action).not.toHaveBeenCalled();
  });

  it('run() is a no-op when the item has no action', async () => {
    const svc = new ContextMenuService();
    await svc.run({ label: 'Nothing' });
    // No throw, no state change. open() is null since we never opened.
    expect(svc.open()).toBeNull();
  });
});
