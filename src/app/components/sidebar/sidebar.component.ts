import { Component, inject } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import { faPlus } from '@fortawesome/free-solid-svg-icons';
import { LibraryService } from '../../services/library.service';

@Component({
  selector: 'app-sidebar',
  imports: [FaIconComponent],
  templateUrl: './sidebar.component.html',
})
export class SidebarComponent {
  protected readonly library = inject(LibraryService);
  protected readonly faPlus = faPlus;

  protected async add(): Promise<void> {
    await this.library.addTrackFromPicker();
  }
}
