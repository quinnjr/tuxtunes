import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenuService, type ContextMenuItem } from '../../services/context-menu.service';
import { DeviceService } from '../../services/device.service';
import { LibraryService, Playlist } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { SidebarComponent, buildPlaylistTree } from './sidebar.component';
import type { Device, DeviceRaw } from '../../models/device';

const RAW_DEVICE: DeviceRaw = {
  id: 11,
  name: 'FiiO M11',
  kind: 'filesystem',
  device_key: 'fs:/mnt/dap',
  key_is_weak: false,
  root_path: '/Music',
  mount_path: '/mnt/dap',
  last_seen_at: '2026-08-31T12:00:00Z',
  last_sync_at: null,
  selection: [],
  layout_template: '{title}.{ext}',
  auto_sync: false,
  mirror_deletes: true,
  write_playlist_objects: true,
};

interface SidebarInternals {
  setView(v: string): void;
  isActive(v: string): boolean;
  onPlaylistClick(node: { playlist: Playlist; children: unknown[] }): void;
  isFolder(node: { playlist: Playlist; children: unknown[] }): boolean;
  onPlaylistContextMenu(node: { playlist: Playlist; children: unknown[] }, event: MouseEvent): void;
  onPlaylistAreaContextMenu(event: MouseEvent): void;
  isExpanded(p: Playlist): boolean;
  openDevice(d: Device): void;
  addDevice(): void;
  onDeviceContextMenu(d: Device, event: MouseEvent): void;
  isAttached(d: Device): boolean;
  rescanDevices(): void;
}

const RAW_PLAYLISTS = [
  {
    id: 5,
    name: 'Rock',
    kind: 'folder',
    parent_id: null,
    sort_order: 1,
    cached_track_count: 0,
    sync_source_id: 1,
  },
  {
    id: 6,
    name: 'Metal',
    kind: 'regular',
    parent_id: 5,
    sort_order: 0,
    cached_track_count: 12,
    sync_source_id: 1,
  },
  {
    id: 7,
    name: 'Chill',
    kind: 'smart',
    parent_id: null,
    sort_order: 2,
    cached_track_count: null,
    sync_source_id: null,
  },
  {
    id: 8,
    name: 'Orphan',
    kind: 'regular',
    parent_id: 999,
    sort_order: 3,
    cached_track_count: 1,
    sync_source_id: null,
  },
];

function pl(over: Partial<Playlist>): Playlist {
  return {
    id: 1,
    name: 'p',
    kind: 'regular',
    parentId: null,
    sortOrder: 0,
    trackCount: 0,
    synced: false,
    ...over,
  };
}

function setup(playlists: unknown[] = [], devices: unknown[] = []) {
  const stub = tauriStub(async (cmd) => {
    if (cmd === 'list_playlists') return playlists;
    if (cmd === 'list_devices' || cmd === 'refresh_devices') return devices;
    return defaultInvoke(cmd);
  });
  TestBed.configureTestingModule({
    imports: [SidebarComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(SidebarComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as SidebarInternals,
    ui: TestBed.inject(UiService),
    library: TestBed.inject(LibraryService),
    devices: TestBed.inject(DeviceService),
    stub,
  };
}

async function settle(fixture: { detectChanges(): void }): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  fixture.detectChanges();
}

describe('buildPlaylistTree', () => {
  it('nests children under folders and keeps sibling order', () => {
    const tree = buildPlaylistTree([
      pl({ id: 1, name: 'A', kind: 'folder' }),
      pl({ id: 2, name: 'A1', parentId: 1 }),
      pl({ id: 3, name: 'B' }),
      pl({ id: 4, name: 'A2', parentId: 1 }),
    ]);
    expect(tree.map((n) => n.playlist.name)).toEqual(['A', 'B']);
    expect(tree[0].children.map((n) => n.playlist.name)).toEqual(['A1', 'A2']);
  });

  it('promotes playlists with a missing parent to the root', () => {
    const tree = buildPlaylistTree([pl({ id: 1, name: 'Lost', parentId: 42 })]);
    expect(tree.map((n) => n.playlist.name)).toEqual(['Lost']);
  });

  it('does not loop on a self-parented row', () => {
    const tree = buildPlaylistTree([pl({ id: 1, name: 'Self', parentId: 1, kind: 'folder' })]);
    expect(tree).toHaveLength(1);
    expect(tree[0].children).toHaveLength(0);
  });

  it('breaks a parent_id cycle by promoting its members to root, keeping non-member children in place', () => {
    // A.parent = B, B.parent = A, C.parent = A. Without the fix, A and
    // B are only ever linked as each other's child and never reach the
    // roots array — the whole trio (including C) silently vanishes.
    const tree = buildPlaylistTree([
      pl({ id: 1, name: 'A', parentId: 2, kind: 'folder' }),
      pl({ id: 2, name: 'B', parentId: 1, kind: 'folder' }),
      pl({ id: 3, name: 'C', parentId: 1 }),
    ]);
    expect(tree.map((n) => n.playlist.name).sort()).toEqual(['A', 'B']);
    const a = tree.find((n) => n.playlist.name === 'A')!;
    const b = tree.find((n) => n.playlist.name === 'B')!;
    expect(a.children.map((n) => n.playlist.name)).toEqual(['C']);
    expect(b.children).toHaveLength(0);
  });
});

describe('SidebarComponent', () => {
  it('setView("genres") opens the column browser and keeps libraryView=tracks', () => {
    const { cmp, ui } = setup();
    cmp.setView('genres');
    expect(ui.libraryView()).toBe('tracks');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('setView() for other views switches libraryView without touching columnBrowser', () => {
    const { cmp, ui } = setup();
    ui.columnBrowserOpen.set(true);
    cmp.setView('albums');
    expect(ui.libraryView()).toBe('albums');
    expect(ui.columnBrowserOpen()).toBe(true);
  });

  it('isActive() resolves the genres pseudo-view via the column browser flag', () => {
    const { cmp, ui } = setup();
    ui.libraryView.set('tracks');
    ui.columnBrowserOpen.set(true);
    expect(cmp.isActive('genres')).toBe(true);
    expect(cmp.isActive('tracks')).toBe(false);
    ui.columnBrowserOpen.set(false);
    expect(cmp.isActive('genres')).toBe(false);
    expect(cmp.isActive('tracks')).toBe(true);
  });

  it('lists playlists from list_playlists on init, folders collapsed', async () => {
    const { fixture, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    expect(stub.invoke).toHaveBeenCalledWith('list_playlists');
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Rock', 'Chill', 'Orphan']);
    expect(el.textContent).not.toContain('No playlists yet');
  });

  it('clicking a folder expands it to reveal children', async () => {
    const { fixture, cmp, library } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const folder = library.playlists().find((p) => p.id === 5)!;
    cmp.onPlaylistClick({ playlist: folder, children: [{}] });
    fixture.detectChanges();
    expect(cmp.isExpanded(folder)).toBe(true);
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Rock', 'Metal', 'Chill', 'Orphan']);
    expect(library.activePlaylistId()).toBeNull();
  });

  it("a 'regular' node with children is treated as a folder (old syncs stored folders that way)", async () => {
    const { fixture, cmp, library, stub } = setup([
      {
        id: 1,
        name: 'Genre',
        kind: 'regular',
        parent_id: null,
        sort_order: 0,
        cached_track_count: 900,
      },
      { id: 2, name: 'Band', kind: 'regular', parent_id: 1, sort_order: 0, cached_track_count: 9 },
    ]);
    await settle(fixture);
    const genre = library.playlists().find((p) => p.id === 1)!;
    const node = { playlist: genre, children: [{}] };
    expect(cmp.isFolder(node)).toBe(true);
    stub.invoke.mockClear();
    cmp.onPlaylistClick(node);
    await settle(fixture);
    expect(cmp.isExpanded(genre)).toBe(true);
    expect(library.activePlaylistId()).toBeNull();
    expect(stub.invoke).not.toHaveBeenCalledWith('open_playlist', expect.anything());
    const el = fixture.nativeElement as HTMLElement;
    const names = [...el.querySelectorAll('[data-playlist-id]')].map((b) => b.textContent?.trim());
    expect(names).toEqual(['Genre', 'Band']);
  });

  it('clicking a playlist opens it in the tracks view and deactivates library views', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    ui.libraryView.set('albums');
    ui.columnBrowserOpen.set(true);
    cmp.onPlaylistClick({ playlist: library.playlists().find((p) => p.id === 6)!, children: [] });
    await settle(fixture);
    expect(ui.libraryView()).toBe('tracks');
    expect(ui.columnBrowserOpen()).toBe(false);
    expect(library.activePlaylistId()).toBe(6);
    expect(stub.invoke).toHaveBeenCalledWith('open_playlist', { playlistId: 6 });
    expect(cmp.isActive('tracks')).toBe(false);
  });

  it('setView() leaves the active playlist and reloads the library list', async () => {
    const { fixture, cmp, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    library.activePlaylistId.set(6);
    stub.invoke.mockClear();
    cmp.setView('tracks');
    await settle(fixture);
    expect(library.activePlaylistId()).toBeNull();
    expect(stub.invoke).toHaveBeenCalledWith('list_tracks', expect.anything());
    expect(cmp.isActive('tracks')).toBe(true);
  });

  it('right-click on a smart playlist offers edit, rename, delete and the New… items', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    const ev = new MouseEvent('contextmenu');
    const smart = library.playlists().find((p) => p.id === 7)!;
    cmp.onPlaylistContextMenu({ playlist: smart, children: [] }, ev);
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items.map((i) => i.label)).toEqual([
      'Edit Smart Playlist…',
      'Rename…',
      '---',
      'Delete',
      '---',
      'New Playlist…',
      'New Smart Playlist…',
    ]);
    await items[0].action?.();
    expect(ui.smartEditor()).toEqual({ playlistId: 7 });
    library.activePlaylistId.set(7);
    await items[3].action?.();
    expect(stub.invoke).toHaveBeenCalledWith('delete_playlist', { playlistId: 7 });
    expect(library.activePlaylistId()).toBeNull();
  });

  it('right-click labels rename and delete identically for synced and user playlists', async () => {
    // Renames and deletes persist across syncs (name override +
    // tombstones), so no "until next sync" qualifier applies anymore.
    const { fixture, cmp, library } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    const ev = new MouseEvent('contextmenu');
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 6)!, children: [] },
      ev,
    );
    const synced = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(synced.map((i) => i.label)).toContain('Delete');
    expect(synced.map((i) => i.label)).toContain('Rename…');
    expect(synced.map((i) => i.label).join(',')).not.toContain('until next sync');

    show.mockClear();
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 8)!, children: [] },
      ev,
    );
    const user = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(user.map((i) => i.label)).toContain('Delete');
    expect(user.map((i) => i.label)).not.toContain('Edit Smart Playlist…');
  });

  it('right-click on a folder offers rename and delete, and New Playlist… targets the folder', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 5)!, children: [{}] },
      new MouseEvent('contextmenu'),
    );
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items.map((i) => i.label)).toContain('Rename…');
    expect(items.map((i) => i.label)).toContain('Delete');

    await items.find((i) => i.label === 'New Playlist…')!.action?.();
    await ui.namePrompt()!.onSubmit('In Folder');
    expect(stub.invoke).toHaveBeenCalledWith('create_playlist', {
      name: 'In Folder',
      parentId: 5,
    });
  });

  it('deleting a folder with children asks for confirmation first', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 5)!, children: [{}] },
      new MouseEvent('contextmenu'),
    );
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    await items.find((i) => i.label === 'Delete')!.action?.();
    expect(stub.invoke).not.toHaveBeenCalledWith('delete_playlist', expect.anything());
    const confirm = ui.confirm();
    expect(confirm).not.toBeNull();
    expect(confirm!.message).toContain('Rock');
    await confirm!.onConfirm();
    expect(stub.invoke).toHaveBeenCalledWith('delete_playlist', { playlistId: 5 });
  });

  it('Rename… opens the name prompt with the current name read at click time', async () => {
    const { fixture, cmp, ui, library, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 7)!, children: [] },
      new MouseEvent('contextmenu'),
    );
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    // A sync-driven refresh may replace the playlists while the menu is
    // open; the prompt must show the current name, not the captured one.
    library.playlists.update((all) =>
      all.map((p) => (p.id === 7 ? { ...p, name: 'Chill (synced)' } : p)),
    );
    await items.find((i) => i.label === 'Rename…')!.action?.();
    expect(ui.namePrompt()?.initial).toBe('Chill (synced)');
    await ui.namePrompt()!.onSubmit('Chillier');
    expect(stub.invoke).toHaveBeenCalledWith('rename_playlist', {
      playlistId: 7,
      name: 'Chillier',
    });
  });

  it('right-click on the empty playlist area offers only the New… items', async () => {
    const { fixture, cmp, ui, stub } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    cmp.onPlaylistAreaContextMenu(new MouseEvent('contextmenu'));
    const items = (show.mock.calls[0][1] ?? []) as ContextMenuItem[];
    expect(items.map((i) => i.label)).toEqual(['New Playlist…', 'New Smart Playlist…']);

    await items[0].action?.();
    expect(ui.namePrompt()?.title).toBe('New Playlist');
    await ui.namePrompt()!.onSubmit('Fresh');
    expect(stub.invoke).toHaveBeenCalledWith('create_playlist', { name: 'Fresh', parentId: null });

    await items[1].action?.();
    expect(ui.smartEditor()).toEqual({ playlistId: null });
  });

  it('a right-click on a playlist row does not also trigger the area menu', async () => {
    const { fixture, cmp, library } = setup(RAW_PLAYLISTS);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    const ev = new MouseEvent('contextmenu');
    const stop = vi.spyOn(ev, 'stopPropagation');
    cmp.onPlaylistContextMenu(
      { playlist: library.playlists().find((p) => p.id === 7)!, children: [] },
      ev,
    );
    expect(show).toHaveBeenCalledTimes(1);
    expect(stop).toHaveBeenCalled();
  });

  it('renders the All Songs / Artists / Albums / Genres buttons', () => {
    const { fixture } = setup();
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    for (const label of ['All Songs', 'Artists', 'Albums', 'Genres']) {
      expect(text).toContain(label);
    }
  });
});

describe('SidebarComponent devices section', () => {
  it('renders a Devices caption', () => {
    const { fixture } = setup();
    expect((fixture.nativeElement as HTMLElement).textContent).toContain('Devices');
  });

  it('shows the empty state when no devices are known', async () => {
    const { fixture } = setup();
    await settle(fixture);
    expect((fixture.nativeElement as HTMLElement).textContent).toContain('No devices connected');
  });

  it('renders one row per known device', async () => {
    const { fixture } = setup([], [RAW_DEVICE]);
    await settle(fixture);
    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('FiiO M11');
    expect(text).not.toContain('No devices connected');
  });

  it('dims a device that has never been seen', async () => {
    const unseen = { ...RAW_DEVICE, last_seen_at: null };
    const { fixture, cmp, devices } = setup([], [unseen]);
    await settle(fixture);
    expect(cmp.isAttached(devices.devices()[0])).toBe(false);
    const row = (fixture.nativeElement as HTMLElement).querySelector('[title*="not connected"]');
    expect(row?.className).toContain('opacity-50');
  });

  it('clicking a device opens the device view and clears the active playlist', async () => {
    const { fixture, cmp, ui, library, devices } = setup([], [RAW_DEVICE]);
    await settle(fixture);
    library.activePlaylistId.set(3);
    cmp.openDevice(devices.devices()[0]);
    expect(ui.libraryView()).toBe('device');
    expect(ui.activeDeviceId()).toBe(11);
    expect(library.activePlaylistId()).toBeNull();
  });

  it('right-clicking a device offers Sync Now, disabled while running', async () => {
    const { fixture, cmp, devices, stub } = setup([], [RAW_DEVICE]);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');

    cmp.onDeviceContextMenu(devices.devices()[0], new MouseEvent('contextmenu'));
    const first = show.mock.calls[0][1] as ContextMenuItem[];
    const labels = first.map((i) => i.label);
    expect(labels).toContain('Sync Now');
    expect(labels).toContain('Forget Device');
    expect(first.find((i) => i.label === 'Sync Now')?.disabled).toBe(false);
    expect(first.find((i) => i.label === 'Cancel Sync')?.disabled).toBe(true);

    stub.emit('device:progress', {
      device_id: 11,
      phase: 'uploading',
      current: 1,
      total: 4,
      message: 'a.flac',
    });
    cmp.onDeviceContextMenu(devices.devices()[0], new MouseEvent('contextmenu'));
    const second = show.mock.calls[1][1] as ContextMenuItem[];
    expect(second.find((i) => i.label === 'Sync Now')?.disabled).toBe(true);
    expect(second.find((i) => i.label === 'Cancel Sync')?.disabled).toBe(false);
  });

  it('Forget Device confirms before removing anything', async () => {
    const { fixture, cmp, ui, devices, stub } = setup([], [RAW_DEVICE]);
    await settle(fixture);
    const ctx = TestBed.inject(ContextMenuService);
    const show = vi.spyOn(ctx, 'show');
    cmp.onDeviceContextMenu(devices.devices()[0], new MouseEvent('contextmenu'));
    const items = show.mock.calls[0][1] as ContextMenuItem[];

    await items.find((i) => i.label === 'Forget Device')?.action?.();
    expect(stub.invoke).not.toHaveBeenCalledWith('forget_device', { deviceId: 11 });
    expect(ui.confirm()?.title).toBe('Forget Device');

    await ui.confirm()!.onConfirm();
    expect(stub.invoke).toHaveBeenCalledWith('forget_device', { deviceId: 11 });
  });

  it('the empty state offers a way to add a device', async () => {
    const { fixture } = setup();
    await settle(fixture);
    const el = (fixture.nativeElement as HTMLElement).querySelector(
      '[data-testid="add-first-device"]',
    );
    expect(el).not.toBeNull();
  });

  it('adding a device invokes the picker and opens the new device', async () => {
    const stubDevices = [{ ...RAW_DEVICE, id: 11 }];
    const { fixture, cmp, ui, stub } = setup([], stubDevices);
    await settle(fixture);
    stub.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'pick_and_add_device') return 11;
      if (cmd === 'list_devices' || cmd === 'refresh_devices') return stubDevices;
      return undefined;
    });

    cmp.addDevice();
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
    fixture.detectChanges();

    expect(stub.invoke).toHaveBeenCalledWith('pick_and_add_device');
    expect(ui.libraryView()).toBe('device');
    expect(ui.activeDeviceId()).toBe(11);
  });

  it('a dismissed picker changes nothing', async () => {
    const { fixture, cmp, ui, stub } = setup();
    await settle(fixture);
    stub.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'pick_and_add_device') return null;
      return [];
    });

    cmp.addDevice();
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
    fixture.detectChanges();

    expect(ui.libraryView()).not.toBe('device');
    expect(ui.activeDeviceId()).toBeNull();
  });

  it('the rescan button re-stats known devices', async () => {
    const { fixture, cmp, stub } = setup([], [RAW_DEVICE]);
    await settle(fixture);
    cmp.rescanDevices();
    await settle(fixture);
    expect(stub.invoke).toHaveBeenCalledWith('refresh_devices');
  });
});
