import {
  Component,
  HostListener,
  effect,
  inject,
  signal,
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

  constructor() {
    // Each newly opened prompt starts from its own initial value.
    effect(() => {
      const req = this.ui.namePrompt();
      this.value.set(req?.initial ?? '');
    });
  }

  @HostListener('document:keydown.escape')
  onEscape(): void {
    if (this.ui.namePrompt() !== null) this.cancel();
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
