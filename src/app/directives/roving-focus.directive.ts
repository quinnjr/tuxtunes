import {
  AfterViewInit,
  Directive,
  ElementRef,
  InjectionToken,
  OnDestroy,
  inject,
  output,
} from '@angular/core';

/** Which arrow keys move focus. Defaults to `both` when not provided. */
export type RovingFocusOrientation = 'horizontal' | 'vertical' | 'both';

export const ROVING_FOCUS_ORIENTATION = new InjectionToken<RovingFocusOrientation>(
  'ROVING_FOCUS_ORIENTATION',
);

/**
 * Keyboard model for the app's single-select composite widgets
 * (`radiogroup`/`radio`, `listbox`/`option`, `menu`/`menuitem`). Native
 * `<button>`s are all tab stops and ignore arrow keys, which is not what
 * these roles promise; this implements the WAI-ARIA pattern: one tab stop
 * (roving `tabindex`), arrows move focus and selection, Home/End jump to
 * the ends. Disabled items are skipped, so a disabled entry can never
 * trap the keyboard.
 *
 * Applied to the *container*. It reads its item children each time, so it
 * needs no knowledge of the `@for` that renders them.
 *
 * Usage:
 * ```html
 * <div role="radiogroup" appRovingFocus (appRovingFocusSelect)="onSelect(i)">
 *   <button role="radio" [attr.aria-checked]="..."> ... </button>
 * </div>
 * ```
 *
 * Selection follows focus, which is the pattern for radios and
 * single-select listboxes.
 */
@Directive({
  selector: '[appRovingFocus]',
  host: {
    '(keydown)': 'onKeydown($event)',
  },
})
export class RovingFocusDirective implements AfterViewInit, OnDestroy {
  /** Index into the FULL item list (disabled entries included), matching
   * what a consumer's `@for` iterates; fires together with focus movement. */
  readonly appRovingFocusSelect = output<number>();

  private readonly orientation = inject(ROVING_FOCUS_ORIENTATION, { optional: true }) ?? 'both';

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private observer: MutationObserver | null = null;

  ngAfterViewInit(): void {
    this.syncTabStop();
    if (typeof MutationObserver === 'undefined') return;
    this.observer = new MutationObserver(() => {
      // Re-assert the invariant when items are added or removed — a menu
      // flyout inserts buttons into this same subtree.
      this.syncTabStop();
    });
    this.observer.observe(this.host.nativeElement, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['aria-checked', 'aria-selected', 'aria-disabled'],
    });
  }

  ngOnDestroy(): void {
    this.observer?.disconnect();
    this.observer = null;
  }

  protected onKeydown(event: Event): void {
    if (!(event instanceof KeyboardEvent)) return;
    const enabled = this.enabledItems();
    if (enabled.length === 0) return;

    const active = this.host.nativeElement.ownerDocument.activeElement as HTMLElement;
    const current = enabled.indexOf(active);
    const horizontal = this.orientation !== 'vertical';
    const vertical = this.orientation !== 'horizontal';

    let next: number;
    switch (event.key) {
      case 'ArrowRight': {
        if (!horizontal) return;
        next = this.step(enabled.length, current, 1);
        break;
      }
      case 'ArrowLeft': {
        if (!horizontal) return;
        next = this.step(enabled.length, current, -1);
        break;
      }
      case 'ArrowDown': {
        if (!vertical) return;
        next = this.step(enabled.length, current, 1);
        break;
      }
      case 'ArrowUp': {
        if (!vertical) return;
        next = this.step(enabled.length, current, -1);
        break;
      }
      case 'Home': {
        next = 0;
        break;
      }
      case 'End': {
        next = enabled.length - 1;
        break;
      }
      default: {
        return;
      }
    }

    event.preventDefault();
    const target = enabled[next];
    target.focus();
    this.setTabStop(target);
    // Emit the index into the FULL item list, which is what a consumer's
    // `@for` iterates (disabled entries included).
    this.appRovingFocusSelect.emit(this.items().indexOf(target));
  }

  /**
   * Next index in `direction`, wrapping. `current === -1` (focus on the
   * container, e.g. a keyboard-opened menu) enters at the appropriate end.
   */
  private step(length: number, current: number, direction: 1 | -1): number {
    if (current === -1) return direction === 1 ? 0 : length - 1;
    return (current + direction + length) % length;
  }

  /**
   * Item nodes in DOM order: elements carrying an interactive ARIA role,
   * excluding disabled ones (a `disabled` button cannot take focus, so
   * including it would make the arrow key a silent no-op). `[role="none"]`
   * wrappers are not items; the buttons inside them are.
   *
   * Scoped to the active menu level: when focus sits inside a
   * `[data-submenu]` flyout, only that flyout's items cycle — Up/Down and
   * Home/End must not walk out into the parent menu and strand the open
   * flyout, and Right/Left (the level switch) belong to the menu's own
   * key handler. The select emit is then relative to that level, which
   * only the context menu exercises and it does not consume the output.
   */
  protected items(): HTMLElement[] {
    const root = this.host.nativeElement;
    const active = root.ownerDocument.activeElement;
    const sub = active instanceof HTMLElement ? active.closest('[data-submenu]') : null;
    const scope = sub !== null && root.contains(sub) ? sub : root;
    const selector =
      '[role="radio"], [role="option"], [role="tab"], [role="menuitem"], [role="menuitemcheckbox"]';
    return [...scope.querySelectorAll<HTMLElement>(selector)];
  }

  private enabledItems(): HTMLElement[] {
    return this.items().filter(
      (el) => !el.hasAttribute('disabled') && el.getAttribute('aria-disabled') !== 'true',
    );
  }

  /**
   * Ensure exactly one tab stop. Prefers the checked/selected item (APG:
   * Tab enters the group at the selected option), else the first enabled
   * item, else the first item. Every other item is forced to `-1`,
   * including ones that never had a `tabindex` attribute — otherwise a
   * native button stays an implicit tab stop and Tab steps through all of
   * them.
   */
  private syncTabStop(): void {
    const items = this.enabledItems();
    if (items.length === 0) {
      this.setTabStop(null);
      return;
    }
    const preferred =
      items.find(
        (el) =>
          el.getAttribute('aria-checked') === 'true' || el.getAttribute('aria-selected') === 'true',
      ) ?? items[0];
    this.setTabStop(preferred);
  }

  /** `null` forces every item to `-1` (no tab stop in the group). */
  private setTabStop(target: HTMLElement | null): void {
    for (const el of this.items()) {
      el.setAttribute('tabindex', el === target ? '0' : '-1');
    }
  }
}
