import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { Choice, ChoiceSelectComponent } from './choice-select.component';

@Component({
  imports: [ChoiceSelectComponent],
  template: `
    <app-choice-select
      label="Sample rate"
      [choices]="choices"
      [value]="value()"
      (changed)="value.set($event)"
    />
  `,
})
class HostComponent {
  readonly choices: readonly Choice<number | null>[] = [
    { value: null, label: 'Same as source' },
    { value: 44_100, label: '44.1 kHz' },
    { value: 96_000, label: '96 kHz' },
  ];
  readonly value = signal<number | null>(96_000);
}

async function setup() {
  TestBed.configureTestingModule({ imports: [HostComponent] });
  const fixture = TestBed.createComponent(HostComponent);
  fixture.detectChanges();
  // ngModel writes its value on a microtask.
  await fixture.whenStable();
  fixture.detectChanges();
  const el = fixture.nativeElement as HTMLElement;
  return { fixture, host: fixture.componentInstance, select: el.querySelector('select')! };
}

function pick(select: HTMLSelectElement, index: number): void {
  select.selectedIndex = index;
  select.dispatchEvent(new Event('change'));
}

describe('ChoiceSelectComponent', () => {
  it('selects the option matching the bound value', async () => {
    const { select } = await setup();
    expect(select.selectedOptions[0]?.textContent).toContain('96 kHz');
  });

  it('emits the typed value of the chosen option', async () => {
    const { fixture, host, select } = await setup();
    pick(select, 1);
    fixture.detectChanges();
    expect(host.value()).toBe(44_100);
  });

  it('carries null through as a real value, not a string', async () => {
    const { fixture, host, select } = await setup();
    pick(select, 0);
    fixture.detectChanges();
    expect(host.value()).toBeNull();
    expect(select.selectedOptions[0]?.textContent).toContain('Same as source');
  });
});
