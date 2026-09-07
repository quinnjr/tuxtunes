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
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
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
    el.querySelector('form')!.dispatchEvent(new Event('submit'));
    fixture.detectChanges();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ui.namePrompt()).not.toBeNull();
  });

  it('focuses the input when opened, including on a second open', async () => {
    const { fixture, el, ui } = setup();
    ui.namePrompt.set({ title: 'New Playlist', initial: '', onSubmit: vi.fn() });
    fixture.detectChanges();
    await new Promise((r) => setTimeout(r));
    expect(document.activeElement).toBe(el.querySelector('input'));

    ui.namePrompt.set(null);
    fixture.detectChanges();
    ui.namePrompt.set({ title: 'Rename Playlist', initial: 'x', onSubmit: vi.fn() });
    fixture.detectChanges();
    await new Promise((r) => setTimeout(r));
    expect(document.activeElement).toBe(el.querySelector('input'));
  });

  it('keystrokes inside the dialog do not reach document-level shortcut listeners', () => {
    const { fixture, el, ui } = setup();
    ui.namePrompt.set({ title: 'New Playlist', initial: '', onSubmit: vi.fn() });
    fixture.detectChanges();
    const docSpy = vi.fn();
    document.addEventListener('keydown', docSpy);
    try {
      el.querySelector('input')!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'q', bubbles: true }),
      );
      expect(docSpy).not.toHaveBeenCalled();
      // Escape must still bubble so the dialog's own document-level
      // dismiss listener sees it.
      el.querySelector('input')!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
      );
      expect(docSpy).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener('keydown', docSpy);
    }
  });

  it('a mousedown on dialog padding keeps focus in the input', async () => {
    const { fixture, el, ui } = setup();
    ui.namePrompt.set({ title: 'New Playlist', initial: '', onSubmit: vi.fn() });
    fixture.detectChanges();
    await new Promise((r) => setTimeout(r));
    const form = el.querySelector('form')!;
    const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    form.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    const buttonDown = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    el.querySelector('button')!.dispatchEvent(buttonDown);
    expect(buttonDown.defaultPrevented).toBe(false);
  });

  it('cancel closes without submitting', () => {
    const { fixture, el, ui } = setup();
    const onSubmit = vi.fn();
    ui.namePrompt.set({ title: 'New Playlist', initial: 'x', onSubmit });
    fixture.detectChanges();
    el.querySelector<HTMLButtonElement>('button[type="button"]')!.click();
    fixture.detectChanges();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ui.namePrompt()).toBeNull();
  });
});
