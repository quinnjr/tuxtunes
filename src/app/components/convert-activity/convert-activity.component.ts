import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import { ConvertService } from '../../services/convert.service';
import { UiService } from '../../services/ui.service';

/**
 * "Converting 2 of 4 · 63%" plus a Cancel button, shown only while a
 * batch is in flight. Rendered in the status bar (always visible) and
 * on the conversion settings page; the host sets text size and layout.
 */
@Component({
  selector: 'app-convert-activity',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (convert.running()) {
      <span>{{ label() }}</span>
      <button
        type="button"
        (click)="cancel()"
        class="rounded px-1.5 text-accent hover:bg-bg-elevated"
      >
        Cancel
      </button>
    }
  `,
})
export class ConvertActivityComponent {
  protected readonly convert = inject(ConvertService);
  private readonly ui = inject(UiService);

  /** Name the track being converted; the status bar has no room for it. */
  readonly showTitle = input(false);

  protected readonly label = computed(this.#computeLabel.bind(this));

  protected async cancel(): Promise<void> {
    await this.ui.guard(this.convert.cancel());
  }

  #computeLabel(): string {
    const p = this.convert.progress();
    // Queued but not yet started: the worker has not said which file.
    if (!p) return 'Converting…';
    const title = this.showTitle() ? `: ${p.title}` : '';
    const pct = p.percent === null ? '' : ` · ${p.percent}%`;
    return `Converting ${p.current + 1} of ${p.total}${title}${pct}`;
  }
}
