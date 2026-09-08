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

function setup() {
  TestBed.configureTestingModule({ imports: [HostComponent] });
  const fixture = TestBed.createComponent(HostComponent);
  fixture.detectChanges();
  const el = fixture.nativeElement as HTMLElement;
  return { fixture, host: fixture.componentInstance, select: el.querySelector('select')! };
}

describe('ChoiceSelectComponent', () => {
  it('selects the option matching the bound value', () => {
    const { select } = setup();
    expect(select.value).toBe('96000');
  });

  it('emits a number for a numeric option', () => {
    const { fixture, host, select } = setup();
    select.value = '44100';
    select.dispatchEvent(new Event('change'));
    fixture.detectChanges();
    expect(host.value()).toBe(44_100);
  });

  it('maps the empty option back to null ("same as source")', () => {
    const { fixture, host, select } = setup();
    select.value = '';
    select.dispatchEvent(new Event('change'));
    fixture.detectChanges();
    expect(host.value()).toBeNull();
    expect(select.selectedOptions[0]?.textContent).toContain('Same as source');
  });
});
