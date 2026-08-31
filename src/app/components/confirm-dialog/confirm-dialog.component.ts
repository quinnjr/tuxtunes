import { Component, HostListener, inject, ChangeDetectionStrategy } from '@angular/core';
import { UiService } from '../../services/ui.service';

/**
 * Small modal for confirming actions that destroy more than what was
 * clicked (deleting a folder full of playlists). Driven by
 * `ui.confirm`.
 */
@Component({
  selector: 'app-confirm-dialog',
  imports: [],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './confirm-dialog.component.html',
})
export class ConfirmDialogComponent {
  protected readonly ui = inject(UiService);

  @HostListener('document:keydown.escape')
  onEscape(): void {
    if (this.ui.confirm() !== null) this.cancel();
  }

  protected async confirm(): Promise<void> {
    const req = this.ui.confirm();
    if (req === null) return;
    this.ui.confirm.set(null);
    await this.ui.guard(Promise.resolve(req.onConfirm()));
  }

  protected cancel(): void {
    this.ui.confirm.set(null);
  }
}
