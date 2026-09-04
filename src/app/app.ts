import { Component, HostListener, OnInit, inject, ChangeDetectionStrategy } from '@angular/core';
import { ConfirmDialogComponent } from './components/confirm-dialog/confirm-dialog.component';
import { ContextMenuComponent } from './components/context-menu/context-menu.component';
import { DeviceDetailComponent } from './components/device-detail/device-detail.component';
import { ImportWizardComponent } from './components/import-wizard/import-wizard.component';
import { MainContentComponent } from './components/main-content/main-content.component';
import { MenuBarComponent } from './components/menu-bar/menu-bar.component';
import { NamePromptComponent } from './components/name-prompt/name-prompt.component';
import { NowPlayingPanelComponent } from './components/now-playing-panel/now-playing-panel.component';
import { PreferencesPanelComponent } from './components/preferences-panel/preferences-panel.component';
import { SidebarComponent } from './components/sidebar/sidebar.component';
import { SmartPlaylistEditorComponent } from './components/smart-playlist-editor/smart-playlist-editor.component';
import { StatusBarComponent } from './components/status-bar/status-bar.component';
import { TrackInfoComponent } from './components/track-info/track-info.component';
import { TransportBarComponent } from './components/transport-bar/transport-bar.component';
import { LibraryService } from './services/library.service';
import { PlaybackService } from './services/playback.service';
import { UiService } from './services/ui.service';
import { WindowService } from './services/window.service';

@Component({
  selector: 'app-root',
  imports: [
    ConfirmDialogComponent,
    ContextMenuComponent,
    DeviceDetailComponent,
    ImportWizardComponent,
    MainContentComponent,
    MenuBarComponent,
    NamePromptComponent,
    NowPlayingPanelComponent,
    PreferencesPanelComponent,
    SidebarComponent,
    SmartPlaylistEditorComponent,
    StatusBarComponent,
    TrackInfoComponent,
    TransportBarComponent,
  ],
  templateUrl: './app.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
  styleUrl: './app.css',
})
export class App implements OnInit {
  private readonly library = inject(LibraryService);
  private readonly playback = inject(PlaybackService);
  protected readonly ui = inject(UiService);
  protected readonly win = inject(WindowService);

  ngOnInit(): void {
    void this.ui.guard(this.library.refreshStats());
  }

  /**
   * The app draws its own context menus; the WebKit default never
   * belongs in the UI. Two exceptions keep it: editable elements
   * (paste, spell-check) and a live text selection — right-click →
   * Copy on selected text (a track title, an error message) has no
   * in-app replacement.
   */
  /**
   * F11 toggles fullscreen on Linux and Windows. The caption button's
   * Alt-click does the same, but many Linux window managers grab
   * Alt+click for their own move/resize before the webview sees it,
   * so a keyboard route is the reliable one.
   */
  @HostListener('document:keydown.F11', ['$event'])
  onFullscreenKey(event: Event): void {
    if (!this.win.customControls()) return;
    event.preventDefault();
    void this.win.toggleFullscreen();
  }

  /**
   * Keyboard media keys (play/pause, stop, next, previous) while the
   * window has focus. Desktops that grab these keys globally route
   * them through MPRIS instead and the webview never sees them; this
   * covers the rest (bare window managers, X11 without a settings
   * daemon, Windows). WebKit reports the play key as either
   * `MediaPlayPause` or a separate `MediaPlay` / `MediaPause` pair
   * depending on the keyboard, so all three are handled.
   */
  @HostListener('document:keydown', ['$event'])
  onMediaKey(event: KeyboardEvent): void {
    const action = mediaKeyAction(event.key);
    if (action === null) return;
    event.preventDefault();
    switch (action) {
      case 'toggle': {
        void this.playback.togglePlay();
        break;
      }
      case 'play': {
        void this.playback.resume();
        break;
      }
      case 'pause': {
        void this.playback.pause();
        break;
      }
      case 'stop': {
        void this.playback.stop();
        break;
      }
      case 'next': {
        void this.playback.next();
        break;
      }
      case 'previous': {
        void this.playback.previous();
        break;
      }
    }
  }

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
    const selection = globalThis.getSelection();
    if (selection !== null && !selection.isCollapsed && selection.toString().length > 0) {
      return;
    }
    event.preventDefault();
  }
}

export type MediaKeyAction = 'toggle' | 'play' | 'pause' | 'stop' | 'next' | 'previous';

/** Map a DOM `KeyboardEvent.key` media-key name to a transport action. */
export function mediaKeyAction(key: string): MediaKeyAction | null {
  switch (key) {
    case 'MediaPlayPause': {
      return 'toggle';
    }
    case 'MediaPlay': {
      return 'play';
    }
    case 'MediaPause': {
      return 'pause';
    }
    case 'MediaStop': {
      return 'stop';
    }
    case 'MediaTrackNext': {
      return 'next';
    }
    case 'MediaTrackPrevious': {
      return 'previous';
    }
    default: {
      return null;
    }
  }
}
