import {
  Component,
  ElementRef,
  HostListener,
  effect,
  inject,
  signal,
  viewChild,
  ChangeDetectionStrategy,
} from '@angular/core';
import { UiService } from '../../services/ui.service';

/**
 * Small modal that asks for a single name — the in-app replacement for
 * `window.prompt` used by "New Playlist…" and "Rename…". Driven by
 * `ui.namePrompt`.
 */
@Component({
  selector: 'app-name-prompt',
  imports: [],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './name-prompt.component.html',
})
export class NamePromptComponent {
  protected readonly ui = inject(UiService);

  protected readonly value = signal('');

  private readonly input = viewChild<ElementRef<HTMLInputElement>>('nameInput');

  constructor() {
    // Each newly opened prompt starts from its own initial value and
    // takes keyboard focus. An `autofocus` attribute is a one-shot,
    // per-document hint that does not fire on repeated dynamic
    // insertion — focus explicitly instead.
    effect(() => {
      const req = this.ui.namePrompt();
      this.value.set(req?.initial ?? '');
      if (req !== null) {
        setTimeout(() => {
          const el = this.input()?.nativeElement;
          el?.focus();
          el?.select();
        });
      }
    });
  }

  @HostListener('document:keydown.escape')
  onEscape(): void {
    if (this.ui.namePrompt() !== null) this.cancel();
  }

  /**
   * Keep keystrokes out of the app's global single-key shortcuts (the
   * Now Playing 'q' toggle and friends) while the dialog is up. Escape
   * still bubbles — the dialog's own dismiss listener is document-level.
   */
  protected onKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Escape') event.stopPropagation();
  }

  /**
   * A click on dialog padding would move focus to the body, where a
   * following keystroke hits global shortcuts instead of the field.
   * Swallow non-interactive mousedowns so the input keeps focus.
   */
  protected onMousedown(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (target?.closest('input, button')) return;
    event.preventDefault();
  }

  protected onInput(event: Event): void {
    this.value.set((event.target as HTMLInputElement).value);
  }

  protected canSubmit(): boolean {
    return this.value().trim().length > 0;
  }

  protected async submit(event: Event): Promise<void> {
    event.preventDefault();
    const req = this.ui.namePrompt();
    const name = this.value().trim();
    if (req === null || name.length === 0) return;
    this.ui.namePrompt.set(null);
    await this.ui.guard(Promise.resolve(req.onSubmit(name)));
  }

  protected cancel(): void {
    this.ui.namePrompt.set(null);
  }
}
