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
import { ContextMenuItem, ContextMenuService } from '../../services/context-menu.service';
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
 *
 * A `parent_id` cycle of length >= 2 (A -> B -> A, or longer) leaves
 * every node in it — and anything hanging off it — unreachable from
 * `roots`, since each is only ever linked as some other cycle member's
 * child. `promoteCycles` finds those unreached nodes, tells cycle
 * members apart from nodes merely hanging off a cycle, and promotes
 * just the cycle members to root (unlinking each from whichever
 * sibling was holding it) so the cycle renders as top-level folders
 * instead of silently vanishing.
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
  promoteCycles(nodes, roots);
  return roots;
}

function promoteCycles(nodes: Map<number, PlaylistNode>, roots: PlaylistNode[]): void {
  const reachedFromRoot = new Set<number>();
  const visit = (list: PlaylistNode[]): void => {
    for (const node of list) {
      if (reachedFromRoot.has(node.playlist.id)) continue;
      reachedFromRoot.add(node.playlist.id);
      visit(node.children);
    }
  };
  visit(roots);

  for (const node of nodes.values()) {
    if (reachedFromRoot.has(node.playlist.id)) continue;
    if (!isCycleMember(node, nodes, reachedFromRoot)) continue;
    const parentId = node.playlist.parentId;
    const parent = parentId === null ? undefined : nodes.get(parentId);
    if (parent) {
      const idx = parent.children.indexOf(node);
      if (idx !== -1) parent.children.splice(idx, 1);
    }
    roots.push(node);
  }
}

/** Walks `node`'s parent chain; true if it leads back to `node` itself. */
function isCycleMember(
  node: PlaylistNode,
  nodes: Map<number, PlaylistNode>,
  reachedFromRoot: ReadonlySet<number>,
): boolean {
  let cursor: PlaylistNode | undefined = node;
  for (let i = 0; i < nodes.size; i++) {
    const parentId: number | null = cursor?.playlist.parentId ?? null;
    cursor = parentId === null ? undefined : nodes.get(parentId);
    if (cursor === node) return true;
    if (cursor === undefined || reachedFromRoot.has(cursor.playlist.id)) return false;
  }
  return false;
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
  private readonly ctx = inject(ContextMenuService);

  protected readonly tree = computed(this.#computeTree.bind(this));

  #computeTree(): PlaylistNode[] {
    return buildPlaylistTree(this.library.playlists());
  }

  /** Folder ids the user has expanded. Folders start collapsed. */
  protected readonly expanded = signal<Set<number>>(new Set<number>());

  constructor() {
    // A finished sync may have added/renamed/removed playlists.
    effect(() => {
      if (this.sync.lastComplete() !== null) void this.ui.guard(this.library.refreshPlaylists());
    });
  }

  ngOnInit(): void {
    void this.ui.guard(this.library.refreshPlaylists());
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
    if (hadPlaylist && this.ui.libraryView() === 'tracks') {
      void this.ui.guard(this.library.refreshTracks());
    }
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

  /**
   * The two creation items, shared by the node and area menus. A
   * folder's menu passes its own id so the new playlist lands inside
   * it; other rows pass their parent so the sibling goes next to them.
   */
  private newPlaylistItems(parentId: number | null): ContextMenuItem[] {
    return [
      {
        label: 'New Playlist…',
        action: () =>
          this.ui.namePrompt.set({
            title: 'New Playlist',
            initial: '',
            onSubmit: async (name) => {
              await this.ui.guard(this.library.createPlaylist(name, parentId));
            },
          }),
      },
      {
        label: 'New Smart Playlist…',
        action: () => this.ui.smartEditor.set({ playlistId: null }),
      },
    ];
  }

  /**
   * The playlists() signal may refresh (a sync finishing) while the
   * menu is open; actions read the row again by id so they never work
   * from a stale snapshot.
   */
  private currentPlaylist(id: number, fallback: Playlist): Playlist {
    return this.library.playlists().find((x) => x.id === id) ?? fallback;
  }

  /**
   * Right-click on a playlist/folder row: rename and delete for
   * everything, edit for smart playlists, plus the creation items.
   * Renames and deletes persist across syncs — the backend records a
   * name override / tombstone so the reconciler honors them.
   */
  protected onPlaylistContextMenu(node: PlaylistNode, event: MouseEvent): void {
    const p = node.playlist;
    const items: ContextMenuItem[] = [];
    if (p.kind === 'smart') {
      items.push({
        label: 'Edit Smart Playlist…',
        action: () => this.ui.smartEditor.set({ playlistId: p.id }),
      });
    }
    items.push(
      {
        label: 'Rename…',
        action: () =>
          this.ui.namePrompt.set({
            title: 'Rename Playlist',
            initial: this.currentPlaylist(p.id, p).name,
            onSubmit: async (name) => {
              await this.ui.guard(this.library.renamePlaylist(p.id, name));
            },
          }),
      },
      { label: '---' },
      {
        label: 'Delete',
        destructive: true,
        action: () => this.deleteWithFolderGuard(node),
      },
      { label: '---' },
      ...this.newPlaylistItems(this.isFolder(node) ? p.id : p.parentId),
    );
    this.ctx.show(event, items);
  }

  /**
   * Deleting a folder orphans its children to the sidebar root — more
   * than the clicked row — so that case confirms first.
   */
  private async deleteWithFolderGuard(node: PlaylistNode): Promise<void> {
    const p = node.playlist;
    if (this.isFolder(node) && node.children.length > 0) {
      const n = node.children.length;
      this.ui.confirm.set({
        title: 'Delete Folder',
        message:
          `Delete the folder “${p.name}”? Its ${n} playlist${n === 1 ? '' : 's'} ` +
          `will move to the top level.`,
        confirmLabel: 'Delete Folder',
        destructive: true,
        onConfirm: async () => {
          await this.ui.guard(this.library.deletePlaylist(p.id));
        },
      });
      return;
    }
    await this.ui.guard(this.library.deletePlaylist(p.id));
  }

  /** Right-click on the playlist section's empty space. */
  protected onPlaylistAreaContextMenu(event: MouseEvent): void {
    this.ctx.show(event, this.newPlaylistItems(null));
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
    void this.ui.guard(this.library.openPlaylist(p.id));
  }
}
