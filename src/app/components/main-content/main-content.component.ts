import { Component, inject, signal } from '@angular/core';
import { LibraryService } from '../../services/library.service';
import { SettingsAudioComponent } from '../settings-audio/settings-audio.component';
import { TrackListViewComponent } from '../track-list-view/track-list-view.component';

type ViewMode = 'tracks' | 'albums' | 'artists' | 'settings';

@Component({
  selector: 'app-main-content',
  imports: [TrackListViewComponent, SettingsAudioComponent],
  templateUrl: './main-content.component.html',
})
export class MainContentComponent {
  protected readonly library = inject(LibraryService);
  protected readonly viewMode = signal<ViewMode>('tracks');
  protected readonly modes: readonly ViewMode[] = [
    'tracks',
    'albums',
    'artists',
    'settings',
  ] as const;

  protected setMode(mode: ViewMode): void {
    this.viewMode.set(mode);
  }
}
