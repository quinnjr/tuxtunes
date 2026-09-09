import { ChangeDetectionStrategy, Component, input, output } from '@angular/core';
import { FormsModule } from '@angular/forms';

/** One `<select>` option. `null` means "keep the source's" — the highest-quality choice. */
export interface Choice<T> {
  value: T;
  label: string;
}

/**
 * A labelled `<select>` over typed choices. `[ngValue]` carries the real
 * values — numbers, `null`, string unions — so nothing is marshalled
 * through strings on either side.
 */
@Component({
  selector: 'app-choice-select',
  imports: [FormsModule],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <label class="flex items-center gap-3 text-body text-text-primary">
      <span class="w-40 shrink-0">{{ label() }}</span>
      <select
        class="mac-btn h-8 flex-1 bg-bg-elevated px-2"
        [ngModel]="value()"
        (ngModelChange)="changed.emit($event)"
      >
        @for (c of choices(); track c.label) {
          <option [ngValue]="c.value">{{ c.label }}</option>
        }
      </select>
    </label>
  `,
})
export class ChoiceSelectComponent<T> {
  readonly label = input.required<string>();
  readonly choices = input.required<readonly Choice<T>[]>();
  readonly value = input.required<T>();
  readonly changed = output<T>();
}
