import { Component, inject, signal } from '@angular/core';
import { LibraryService } from '../../services/library.service';

type ViewMode = 'tracks' | 'albums' | 'artists';

@Component({
  selector: 'app-main-content',
  imports: [],
  templateUrl: './main-content.component.html',
})
export class MainContentComponent {
  protected readonly library = inject(LibraryService);
  protected readonly viewMode = signal<ViewMode>('tracks');
  protected readonly modes: readonly ViewMode[] = ['tracks', 'albums', 'artists'] as const;

  protected setMode(mode: ViewMode): void {
    this.viewMode.set(mode);
  }
}
