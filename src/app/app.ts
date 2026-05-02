import { Component, OnInit, inject } from '@angular/core';
import { ContextMenuComponent } from './components/context-menu/context-menu.component';
import { ImportWizardComponent } from './components/import-wizard/import-wizard.component';
import { MainContentComponent } from './components/main-content/main-content.component';
import { NowPlayingPanelComponent } from './components/now-playing-panel/now-playing-panel.component';
import { PreferencesPanelComponent } from './components/preferences-panel/preferences-panel.component';
import { SidebarComponent } from './components/sidebar/sidebar.component';
import { StatusBarComponent } from './components/status-bar/status-bar.component';
import { TransportBarComponent } from './components/transport-bar/transport-bar.component';
import { LibraryService } from './services/library.service';

@Component({
  selector: 'app-root',
  imports: [
    ContextMenuComponent,
    ImportWizardComponent,
    MainContentComponent,
    NowPlayingPanelComponent,
    PreferencesPanelComponent,
    SidebarComponent,
    StatusBarComponent,
    TransportBarComponent,
  ],
  templateUrl: './app.html',
  styleUrl: './app.css',
})
export class App implements OnInit {
  private readonly library = inject(LibraryService);

  ngOnInit(): void {
    void this.library.refreshStats();
  }
}
