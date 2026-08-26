import { NgTemplateOutlet } from '@angular/common';
import {
  Component,
  OnInit,
  computed,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { LibraryService, Playlist } from '../../services/library.service';
import { SyncService } from '../../services/sync.service';
import { LibraryView, UiService } from '../../services/ui.service';

/** A playlist plus its (already-sorted) children, for the sidebar tree. */
export interface PlaylistNode {
  playlist: Playlist;
  children: PlaylistNode[];
}

/**
 * Build a folder tree from the flat playlist list. Siblings keep the
 * backend's order (sort_order, then name). A playlist whose parent is
 * missing is promoted to the root rather than dropped, so a stale
 * parent link never hides a playlist.
 */
export function buildPlaylistTree(playlists: readonly Playlist[]): PlaylistNode[] {
  const nodes = new Map<number, PlaylistNode>();
  for (const p of playlists) nodes.set(p.id, { playlist: p, children: [] });
  const roots: PlaylistNode[] = [];
  for (const node of nodes.values()) {
    const parentId = node.playlist.parentId;
    const parent = parentId === null ? undefined : nodes.get(parentId);
    if (parent && parent !== node) parent.children.push(node);
    else roots.push(node);
  }
  return roots;
}

@Component({
  selector: 'app-sidebar',
  imports: [NgTemplateOutlet],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './sidebar.component.html',
})
export class SidebarComponent implements OnInit {
  protected readonly ui = inject(UiService);
  protected readonly library = inject(LibraryService);
  private readonly sync = inject(SyncService);

  protected readonly tree = computed(this.#computeTree.bind(this));

  #computeTree(): PlaylistNode[] {
    return buildPlaylistTree(this.library.playlists());
  }

  /** Folder ids the user has expanded. Folders start collapsed. */
  protected readonly expanded = signal<Set<number>>(new Set<number>());

  constructor() {
    // A finished sync may have added/renamed/removed playlists.
    effect(() => {
      if (this.sync.lastComplete() !== null) void this.library.refreshPlaylists();
    });
  }

  ngOnInit(): void {
    void this.library.refreshPlaylists();
  }

  /**
   * "Genres" is a tracks view with the Column Browser pre-opened so the
   * user lands on a genre-pivoted list. Other views just switch the
   * top-level libraryView and leave the browser state alone. Any
   * library view leaves the active playlist.
   */
  protected setView(view: LibraryView): void {
    const hadPlaylist = this.library.activePlaylistId() !== null;
    this.library.activePlaylistId.set(null);
    if (view === 'genres') {
      this.ui.libraryView.set('tracks');
      this.ui.columnBrowserOpen.set(true);
    } else {
      this.ui.libraryView.set(view);
    }
    // The track list only fetches on mount; when it stays mounted we
    // must reload it ourselves to drop the playlist's rows.
    if (hadPlaylist && this.ui.libraryView() === 'tracks') void this.library.refreshTracks();
  }

  protected isActive(view: LibraryView): boolean {
    if (this.library.activePlaylistId() !== null) return false;
    if (view === 'genres') {
      return this.ui.libraryView() === 'tracks' && this.ui.columnBrowserOpen();
    }
    return this.ui.libraryView() === view && !this.ui.columnBrowserOpen();
  }

  /**
   * A node is a folder if the backend says so or if it has children —
   * an older sync could have stored a folder as 'regular'.
   */
  protected isFolder(node: PlaylistNode): boolean {
    return node.playlist.kind === 'folder' || node.children.length > 0;
  }

  protected isPlaylistActive(p: Playlist): boolean {
    return this.library.activePlaylistId() === p.id;
  }

  protected isExpanded(p: Playlist): boolean {
    return this.expanded().has(p.id);
  }

  protected toggleFolder(p: Playlist): void {
    this.expanded.update((cur) => {
      const next = new Set(cur);
      if (next.has(p.id)) next.delete(p.id);
      else next.add(p.id);
      return next;
    });
  }

  /** Folders expand/collapse; playlists open in the track list. */
  protected onPlaylistClick(node: PlaylistNode): void {
    const p = node.playlist;
    if (this.isFolder(node)) {
      this.toggleFolder(p);
      return;
    }
    this.ui.libraryView.set('tracks');
    this.ui.columnBrowserOpen.set(false);
    void this.library.openPlaylist(p.id);
  }
}
