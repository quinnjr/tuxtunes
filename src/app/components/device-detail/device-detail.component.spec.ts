import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { DeviceRaw } from '../../models/device';
import { DeviceService } from '../../services/device.service';
import { LibraryService } from '../../services/library.service';
import { UiService } from '../../services/ui.service';
import { appProviders, defaultInvoke, tauriStub } from '../../test-helpers';
import { DeviceDetailComponent } from './device-detail.component';

const RAW_DEVICE: DeviceRaw = {
  id: 1,
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

const RAW_PLAYLISTS = [
  {
    id: 5,
    name: 'Favourites',
    kind: 'regular',
    parent_id: null,
    sort_order: 0,
    cached_track_count: 3,
    sync_source_id: null,
  },
  {
    id: 6,
    name: 'Recently Added',
    kind: 'smart',
    parent_id: null,
    sort_order: 1,
    cached_track_count: 9,
    sync_source_id: null,
  },
  {
    id: 7,
    name: 'A Folder',
    kind: 'folder',
    parent_id: null,
    sort_order: 2,
    cached_track_count: 0,
    sync_source_id: null,
  },
];

const PLAN = {
  adds: 3,
  replaces: 1,
  unchanged: 40,
  deletes: 2,
  skips: 1,
  bytes_out: 1_500_000,
  free_bytes: 900_000_000,
  total_bytes: 1_000_000_000,
};

async function setup(devices: DeviceRaw[] = [RAW_DEVICE]) {
  const stub = tauriStub(async (cmd) => {
    if (cmd === 'list_devices' || cmd === 'refresh_devices') return devices;
    if (cmd === 'list_playlists') return RAW_PLAYLISTS;
    if (cmd === 'preview_device_sync') return PLAN;
    return defaultInvoke(cmd);
  });
  TestBed.configureTestingModule({
    imports: [DeviceDetailComponent],
    providers: appProviders(stub),
  });
  const ui = TestBed.inject(UiService);
  const svc = TestBed.inject(DeviceService);
  await svc.refresh();
  // Playlists come from LibraryService; load them the same way the
  // sidebar does before the view renders.
  await TestBed.inject(LibraryService).refreshPlaylists();
  ui.activeDeviceId.set(devices[0]?.id ?? null);

  const fixture = TestBed.createComponent(DeviceDetailComponent);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  return { fixture, stub, ui, svc };
}

function text(fixture: { nativeElement: HTMLElement }): string {
  return fixture.nativeElement.textContent ?? '';
}

function byTestId<T extends HTMLElement>(
  fixture: { nativeElement: HTMLElement },
  id: string,
): T | null {
  return fixture.nativeElement.querySelector<T>(`[data-testid="${id}"]`);
}

describe('DeviceDetailComponent', () => {
  it('renders the active device name and location', async () => {
    const { fixture } = await setup();
    expect(byTestId(fixture, 'device-name')?.textContent).toContain('FiiO M11');
    expect(text(fixture)).toContain('/mnt/dap');
  });

  it('shows a placeholder when no device is active', async () => {
    const { fixture, ui } = await setup();
    ui.activeDeviceId.set(null);
    fixture.detectChanges();
    expect(byTestId(fixture, 'no-device')).not.toBeNull();
  });

  it('lists playlists and smart playlists but never folders', async () => {
    const { fixture } = await setup();
    expect(byTestId(fixture, 'select-5')).not.toBeNull();
    expect(byTestId(fixture, 'select-6')).not.toBeNull();
    expect(byTestId(fixture, 'select-7')).toBeNull();
  });

  it('ticking a playlist sends that entry to update_device_selection', async () => {
    const { fixture, stub } = await setup();
    byTestId<HTMLInputElement>(fixture, 'select-5')?.click();
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_device_selection', {
      deviceId: 1,
      selection: [{ kind: 'playlist', id: 5 }],
    });
  });

  it('a smart playlist is selected with the smart kind', async () => {
    const { fixture, stub } = await setup();
    byTestId<HTMLInputElement>(fixture, 'select-6')?.click();
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_device_selection', {
      deviceId: 1,
      selection: [{ kind: 'smart', id: 6 }],
    });
  });

  it('unticking a selected playlist removes just that entry', async () => {
    const selected: DeviceRaw = {
      ...RAW_DEVICE,
      selection: [
        { kind: 'playlist', id: 5 },
        { kind: 'smart', id: 6 },
      ],
    };
    const { fixture, stub } = await setup([selected]);
    byTestId<HTMLInputElement>(fixture, 'select-5')?.click();
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_device_selection', {
      deviceId: 1,
      selection: [{ kind: 'smart', id: 6 }],
    });
  });

  it('Sync is disabled until something is selected', async () => {
    const { fixture } = await setup();
    expect(byTestId<HTMLButtonElement>(fixture, 'run-sync')?.disabled).toBe(true);
  });

  it('Sync is enabled once a playlist is selected', async () => {
    const selected: DeviceRaw = { ...RAW_DEVICE, selection: [{ kind: 'playlist', id: 5 }] };
    const { fixture } = await setup([selected]);
    expect(byTestId<HTMLButtonElement>(fixture, 'run-sync')?.disabled).toBe(false);
  });

  it('Preview renders the plan summary', async () => {
    const { fixture, stub } = await setup();
    const buttons = (fixture.nativeElement as HTMLElement).querySelectorAll('button');
    for (const button of buttons) {
      if (button.textContent?.trim() === 'Preview') button.click();
    }
    await fixture.whenStable();
    fixture.detectChanges();

    expect(stub.invoke).toHaveBeenCalledWith('preview_device_sync', { deviceId: 1 });
    const summary = byTestId(fixture, 'plan-summary')?.textContent ?? '';
    expect(summary).toContain('3 to copy bit-exact');
    expect(summary).toContain('2 to remove');
    expect(summary).toContain('1.5 MB to transfer');
  });

  it('Cancel replaces Sync only while a run is in flight', async () => {
    const { fixture, stub } = await setup();
    expect(byTestId(fixture, 'cancel-sync')).toBeNull();

    stub.emit('device:progress', {
      device_id: 1,
      phase: 'uploading',
      current: 1,
      total: 4,
      message: 'a.flac',
    });
    fixture.detectChanges();

    expect(byTestId(fixture, 'run-sync')).toBeNull();
    byTestId<HTMLButtonElement>(fixture, 'cancel-sync')?.click();
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('cancel_device_sync', { deviceId: 1 });
  });

  it('shows progress with a floored percentage', async () => {
    const { fixture, stub } = await setup();
    stub.emit('device:progress', {
      device_id: 1,
      phase: 'uploading',
      current: 1,
      total: 3,
      message: 'a.flac',
    });
    fixture.detectChanges();
    expect(byTestId(fixture, 'progress')?.textContent).toContain('33%');
  });

  it('renders warnings as they arrive', async () => {
    const { fixture, stub } = await setup();
    stub.emit('device:warning', {
      device_id: 1,
      kind: 'unsupported_codec',
      detail: 'track 4: ape',
    });
    fixture.detectChanges();
    expect(byTestId(fixture, 'warnings')?.textContent).toContain('track 4: ape');
  });

  it('summarises the last completed run', async () => {
    const { fixture, stub } = await setup();
    stub.emit('device:complete', {
      device_id: 1,
      added: 3,
      replaced: 0,
      unchanged: 2,
      deleted: 1,
      playlists_written: 1,
      skipped: 0,
      bytes_written: 2_000_000,
    });
    fixture.detectChanges();
    const summary = byTestId(fixture, 'last-complete')?.textContent ?? '';
    expect(summary).toContain('3 added');
    expect(summary).toContain('1 playlist written');
    expect(summary).toContain('2.0 MB transferred');
  });

  it('toggling mirror deletes persists the whole settings row', async () => {
    const { fixture, stub } = await setup();
    byTestId<HTMLInputElement>(fixture, 'mirror-deletes')?.click();
    await fixture.whenStable();
    expect(stub.invoke).toHaveBeenCalledWith('update_device_settings', {
      deviceId: 1,
      settings: {
        name: 'FiiO M11',
        root_path: '/Music',
        layout_template: '{title}.{ext}',
        auto_sync: false,
        mirror_deletes: false,
        write_playlist_objects: true,
      },
    });
  });

  it('explains that removal stays off for a device with a weak key', async () => {
    const weak: DeviceRaw = { ...RAW_DEVICE, key_is_weak: true };
    const { fixture } = await setup([weak]);
    expect(text(fixture)).toContain('no serial number');
  });
});
