import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { ConfirmDialogComponent } from './confirm-dialog.component';

function setup() {
  TestBed.configureTestingModule({
    imports: [ConfirmDialogComponent],
    providers: appProviders(tauriStub()),
  });
  const fixture = TestBed.createComponent(ConfirmDialogComponent);
  fixture.detectChanges();
  return {
    fixture,
    el: fixture.nativeElement as HTMLElement,
    ui: TestBed.inject(UiService),
  };
}

function open(ui: UiService, onConfirm = vi.fn()) {
  ui.confirm.set({
    title: 'Delete Folder',
    message: 'Delete the folder “Rock”?',
    confirmLabel: 'Delete Folder',
    destructive: true,
    onConfirm,
  });
  return onConfirm;
}

describe('ConfirmDialogComponent', () => {
  it('renders nothing while no confirmation is requested', () => {
    const { el } = setup();
    expect(el.querySelector('button')).toBeNull();
  });

  it('shows title, message, and the confirm label', () => {
    const { fixture, el, ui } = setup();
    open(ui);
    fixture.detectChanges();
    expect(el.textContent).toContain('Delete Folder');
    expect(el.textContent).toContain('Rock');
  });

  it('confirming runs the action and closes', async () => {
    const { fixture, el, ui } = setup();
    const onConfirm = open(ui);
    fixture.detectChanges();
    const buttons = [...el.querySelectorAll('button')];
    buttons.find((b) => b.textContent?.includes('Delete Folder'))!.click();
    await fixture.whenStable();
    expect(onConfirm).toHaveBeenCalled();
    expect(ui.confirm()).toBeNull();
  });

  it('cancel closes without running the action', () => {
    const { fixture, el, ui } = setup();
    const onConfirm = open(ui);
    fixture.detectChanges();
    const buttons = [...el.querySelectorAll('button')];
    buttons.find((b) => b.textContent?.includes('Cancel'))!.click();
    fixture.detectChanges();
    expect(onConfirm).not.toHaveBeenCalled();
    expect(ui.confirm()).toBeNull();
  });
});
