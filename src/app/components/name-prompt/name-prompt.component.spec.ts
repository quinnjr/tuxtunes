import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { NamePromptComponent } from './name-prompt.component';

function setup() {
  TestBed.configureTestingModule({
    imports: [NamePromptComponent],
    providers: appProviders(tauriStub()),
  });
  const fixture = TestBed.createComponent(NamePromptComponent);
  fixture.detectChanges();
  return {
    fixture,
    el: fixture.nativeElement as HTMLElement,
    ui: TestBed.inject(UiService),
  };
}

describe('NamePromptComponent', () => {
  it('renders nothing while no prompt is requested', () => {
    const { el } = setup();
    expect(el.querySelector('input')).toBeNull();
  });

  it('shows the title and prefills the input when opened', () => {
    const { fixture, el, ui } = setup();
    ui.namePrompt.set({ title: 'Rename Playlist', initial: 'Mix', onSubmit: vi.fn() });
    fixture.detectChanges();
    expect(el.textContent).toContain('Rename Playlist');
    expect(el.querySelector('input')!.value).toBe('Mix');
  });

  it('submits the trimmed name and closes', () => {
    const { fixture, el, ui } = setup();
    const onSubmit = vi.fn();
    ui.namePrompt.set({ title: 'New Playlist', initial: '', onSubmit });
    fixture.detectChanges();
    const input = el.querySelector('input')!;
    input.value = '  My Mix  ';
    input.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    (el.querySelector('form') as HTMLFormElement).dispatchEvent(new Event('submit'));
    fixture.detectChanges();
    expect(onSubmit).toHaveBeenCalledWith('My Mix');
    expect(ui.namePrompt()).toBeNull();
  });

  it('does not submit a blank name', () => {
    const { fixture, el, ui } = setup();
    const onSubmit = vi.fn();
    ui.namePrompt.set({ title: 'New Playlist', initial: '', onSubmit });
    fixture.detectChanges();
    const input = el.querySelector('input')!;
    input.value = '   ';
    input.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    (el.querySelector('form') as HTMLFormElement).dispatchEvent(new Event('submit'));
    fixture.detectChanges();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ui.namePrompt()).not.toBeNull();
  });

  it('cancel closes without submitting', () => {
    const { fixture, el, ui } = setup();
    const onSubmit = vi.fn();
    ui.namePrompt.set({ title: 'New Playlist', initial: 'x', onSubmit });
    fixture.detectChanges();
    (el.querySelector('button[type="button"]') as HTMLButtonElement).click();
    fixture.detectChanges();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ui.namePrompt()).toBeNull();
  });
});
