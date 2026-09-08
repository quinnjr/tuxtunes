import { ChangeDetectionStrategy, Component, input, output } from '@angular/core';

/** One `<select>` option. `null` means "keep the source's" — the highest-quality choice. */
export interface Choice<T> {
  value: T;
  label: string;
}

/**
 * A labelled `<select>` over numeric choices where `null` is a legal
 * value. `<select>` only speaks strings, so the empty string stands in
 * for `null` on the way in and is mapped back on the way out.
 */
@Component({
  selector: 'app-choice-select',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <label class="flex items-center gap-3 text-body text-text-primary">
      <span class="w-40 shrink-0">{{ label() }}</span>
      <select
        class="mac-btn h-8 flex-1 bg-bg-elevated px-2"
        (change)="onChange($any($event.target).value)"
      >
        @for (c of choices(); track c.label) {
          <option [value]="c.value ?? ''" [selected]="c.value === value()">{{ c.label }}</option>
        }
      </select>
    </label>
  `,
})
export class ChoiceSelectComponent {
  readonly label = input.required<string>();
  readonly choices = input.required<readonly Choice<number | null>[]>();
  readonly value = input.required<number | null>();
  readonly changed = output<number | null>();

  protected onChange(raw: string): void {
    this.changed.emit(raw === '' ? null : Number(raw));
  }
}
