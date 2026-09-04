import { DOCUMENT } from '@angular/common';
import {
  Component,
  DestroyRef,
  HostListener,
  OnInit,
  inject,
  ChangeDetectionStrategy,
} from '@angular/core';
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
  private readonly document = inject(DOCUMENT);
  private readonly destroyRef = inject(DestroyRef);

  constructor() {
    // Capture phase: dialogs stop keydown propagation to keep their
    // text-entry shortcuts local, and a bubble-phase HostListener
    // would never see a transport key pressed while one is open.
    this.document.addEventListener('keydown', this.onMediaKey, { capture: true });
    this.destroyRef.onDestroy(() =>
      this.document.removeEventListener('keydown', this.onMediaKey, { capture: true }),
    );
  }
  protected readonly ui = inject(UiService);
  protected readonly win = inject(WindowService);

  ngOnInit(): void {
    void this.ui.guard(this.library.refreshStats());
  }

  /**
   * Keyboard media keys while the window has focus. Desktops that grab
   * these keys globally route them through MPRIS and the webview never
   * sees them; this covers bare compositors, X11 without a settings
   * daemon, and Windows. WebKitGTK reports the single play/pause key as
   * `MediaPlay` (its toggle identity survives only in `event.code`),
   * Chromium as `MediaPlayPause`; both mean toggle, as they do for every
   * media-key daemon. Auto-repeat is ignored so a held key acts once.
   */
  readonly onMediaKey = (event: KeyboardEvent): void => {
    const run = MEDIA_KEYS[event.key];
    if (run === undefined) return;
    event.preventDefault();
    if (event.repeat) return;
    void run(this.playback);
  };

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
   * The app draws its own context menus; the WebKit default never
   * belongs in the UI. Two exceptions keep it: editable elements
   * (paste, spell-check) and a live text selection — right-click →
   * Copy on selected text (a track title, an error message) has no
   * in-app replacement.
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
    const selection = globalThis.getSelection();
    if (selection !== null && !selection.isCollapsed && selection.toString().length > 0) {
      return;
    }
    event.preventDefault();
  }
}

/** `KeyboardEvent.key` media-key names and the transport action each drives. */
export const MEDIA_KEYS: Record<string, (playback: PlaybackService) => Promise<unknown>> = {
  MediaPlayPause: (p) => p.togglePlay(),
  MediaPlay: (p) => p.togglePlay(),
  MediaPause: (p) => p.pause(),
  MediaStop: (p) => p.stop(),
  MediaTrackNext: (p) => p.next(),
  MediaTrackPrevious: (p) => p.previous(),
};
