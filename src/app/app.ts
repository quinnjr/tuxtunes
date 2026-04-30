import { Component, OnInit, inject } from '@angular/core';
import { ImportWizardComponent } from './components/import-wizard/import-wizard.component';
import { MainContentComponent } from './components/main-content/main-content.component';
import { SidebarComponent } from './components/sidebar/sidebar.component';
import { TransportBarComponent } from './components/transport-bar/transport-bar.component';
import { LibraryService } from './services/library.service';

@Component({
  selector: 'app-root',
  imports: [ImportWizardComponent, MainContentComponent, SidebarComponent, TransportBarComponent],
  templateUrl: './app.html',
  styleUrl: './app.css',
})
export class App implements OnInit {
  private readonly library = inject(LibraryService);

  ngOnInit(): void {
    void this.library.refreshStats();
  }
}
