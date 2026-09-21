import {
  Directive,
  ElementRef,
  OnDestroy,
  OnInit,
  booleanAttribute,
  inject,
  input,
  output,
} from '@angular/core';

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * Shared semantics for the app's sheet and popover surfaces: marks the
 * host as a dialog, keeps Tab focus inside until it closes, and emits
 * `appModalSheetEscape` on Escape.
 *
 * By default it is modal: `aria-modal="true"` tells assistive tech the
 * rest of the page is unavailable, and the app shell is made `inert`
 * while any modal is open (see `app.html`). Pass `[appModalSheetModal]="false"`
 * for an anchored popover (the column picker) that keeps the dialog role
 * and focus trap without claiming modality.
 *
 * Applied to the panel itself (the element carrying `mac-sheet`), so the
 * host is what `aria-modal` refers to. Give it `aria-labelledby` pointing
 * at the heading.
 *
 * Usage:
 * ```html
 * <div class="mac-sheet ..." appModalSheet aria-labelledby="sheet-title"
 *      (appModalSheetEscape)="hide()"> ... </div>
 * ```
 *
 * Self-contained on purpose: it does not require the consuming
 * component to import CDK's A11yModule, and it restores focus to the
 * element that opened the sheet on destroy, so closing returns the user
 * to the control they came from. Escape is scoped to the host rather
 * than bound on document, so one sheet cannot steal the key from
 * another surface legitimately on top of it.
 */
@Directive({
  selector: '[appModalSheet]',
  host: {
    role: 'dialog',
    '[attr.aria-modal]': 'modal() ? "true" : null',
    tabindex: '-1',
    '(keydown)': 'onKeydown($event)',
  },
})
export class ModalSheetDirective implements OnInit, OnDestroy {
  readonly appModalSheetEscape = output<void>();

  /** False for non-modal popovers; see the class doc. */
  readonly modal = input(true, { alias: 'appModalSheetModal', transform: booleanAttribute });

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  /** Element focused when the sheet opened; refocused on destroy. */
  previouslyFocused: HTMLElement | null = null;

  ngOnInit(): void {
    const doc = this.host.nativeElement.ownerDocument;
    const active = doc.activeElement;
    // `activeElement` is the body when the sheet is opened from a menu
    // that just unmounted its trigger, or programmatically. Only a real
    // element is worth restoring to.
    this.previouslyFocused = active instanceof HTMLElement && active !== doc.body ? active : null;
    // Move focus into the sheet so the trap and Escape have somewhere to
    // start; the first focusable child, else the panel itself.
    const [first] = this.focusable();
    (first ?? this.host.nativeElement).focus();
  }

  ngOnDestroy(): void {
    // Restore where the user was before the sheet opened. Deferred a
    // microtask: Angular removes the sheet's subtree *after* this hook,
    // and tearing down the currently-focused node would otherwise drop
    // focus to <body> and undo the restore. `focus()` on a detached
    // node is a no-op, so no isConnected guard is needed. The restore is
    // conditional: if closing this sheet opened another one (its ngOnInit
    // already moved focus into it), yanking focus back here would strand
    // it behind the new modal.
    const previous = this.previouslyFocused;
    queueMicrotask(() => {
      const doc = this.host.nativeElement.ownerDocument;
      if (doc.activeElement !== doc.body) return;
      if (previous === null || !previous.isConnected || previous.closest('[inert]') !== null) {
        return;
      }
      previous.focus();
    });
  }

  protected onKeydown(event: Event): void {
    if (!(event instanceof KeyboardEvent)) return;
    if (event.key === 'Escape') {
      // Deliberately not stopPropagation: the rest of the app keeps
      // document-level Escape listeners working (dialogs that also own
      // an escape handler, the global shortcut guard). Closing is
      // idempotent, so a double-fire is harmless.
      event.preventDefault();
      this.appModalSheetEscape.emit();
      return;
    }
    // A non-modal popover (the column picker) keeps the dialog role,
    // auto-focus, and Escape, but Tab must be free to leave it — trapping
    // a non-modal surface contradicts the `dialog`-without-`aria-modal`
    // contract.
    if (event.key === 'Tab' && this.modal()) this.trapTab(event);
  }

  private trapTab(event: KeyboardEvent): void {
    const items = this.focusable();
    if (items.length === 0) return;
    const first = items[0];
    const last = items.at(-1);
    const active = this.host.nativeElement.ownerDocument.activeElement;
    const index = items.indexOf(active as HTMLElement);

    if (event.shiftKey) {
      // Shift+Tab from the first item, or from the panel itself / outside
      // the list (e.g. focus on the `tabindex="-1"` host after a padding
      // click), wraps to the last item instead of leaving the sheet.
      if (index <= 0) {
        event.preventDefault();
        last?.focus();
      }
    } else if (index === items.length - 1 || index === -1) {
      // Forward Tab from the last item, or from a non-listed node.
      event.preventDefault();
      first.focus();
    }
  }

  private focusable(): HTMLElement[] {
    // Filter out what the user cannot actually reach: hidden inputs and
    // elements inside a `hidden`/`display:none` subtree. `offsetParent`
    // would be simpler but it is null for any `position: fixed`
    // ancestor — i.e. every sheet — and in jsdom.
    return [...this.host.nativeElement.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => !el.closest('[hidden]') && el.style.display !== 'none',
    );
  }
}
