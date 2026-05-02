import { Component, inject } from '@angular/core';
import { LibraryView, UiService } from '../../services/ui.service';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';
import { TrackListViewComponent } from '../track-list-view/track-list-view.component';

@Component({
  selector: 'app-main-content',
  imports: [TrackListViewComponent, SettingsAudioComponent],
  templateUrl: './main-content.component.html',
})
export class MainContentComponent {
  private readonly ui = inject(UiService);
  protected readonly viewMode = this.ui.libraryView;
  protected readonly modes: readonly LibraryView[] = [
    'tracks',
    'albums',
    'artists',
    'settings',
  ] as const;

  protected setMode(mode: LibraryView): void {
    this.ui.libraryView.set(mode);
  }
}
