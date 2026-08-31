import { Component, HostListener, OnInit, inject, ChangeDetectionStrategy } from '@angular/core';
import { ContextMenuComponent } from './components/context-menu/context-menu.component';
import { ImportWizardComponent } from './components/import-wizard/import-wizard.component';
import { MainContentComponent } from './components/main-content/main-content.component';
import { MenuBarComponent } from './components/menu-bar/menu-bar.component';
import { NamePromptComponent } from './components/name-prompt/name-prompt.component';
import { NowPlayingPanelComponent } from './components/now-playing-panel/now-playing-panel.component';
import { PreferencesPanelComponent } from './components/preferences-panel/preferences-panel.component';
import { SidebarComponent } from './components/sidebar/sidebar.component';
import { SmartPlaylistEditorComponent } from './components/smart-playlist-editor/smart-playlist-editor.component';
import { StatusBarComponent } from './components/status-bar/status-bar.component';
import { TransportBarComponent } from './components/transport-bar/transport-bar.component';
import { LibraryService } from './services/library.service';
import { UiService } from './services/ui.service';

@Component({
  selector: 'app-root',
  imports: [
    ContextMenuComponent,
    ImportWizardComponent,
    MainContentComponent,
    MenuBarComponent,
    NamePromptComponent,
    NowPlayingPanelComponent,
    PreferencesPanelComponent,
    SidebarComponent,
    SmartPlaylistEditorComponent,
    StatusBarComponent,
    TransportBarComponent,
  ],
  templateUrl: './app.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
  styleUrl: './app.css',
})
export class App implements OnInit {
  private readonly library = inject(LibraryService);
  private readonly ui = inject(UiService);

  ngOnInit(): void {
    void this.ui.guard(this.library.refreshStats());
  }

  /**
   * The app draws its own context menus; the WebKit default never
   * belongs in the UI. Editable elements keep it — paste, spell-check
   * and friends are genuinely useful there.
   */
  @HostListener('document:contextmenu', ['$event'])
  onDocumentContextMenu(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target?.isContentEditable
    ) {
      return;
    }
    event.preventDefault();
  }
}
