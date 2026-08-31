import { Injector, runInInjectionContext } from '@angular/core';
import { describe, expect, it, vi } from 'vitest';
import { formatBytes, type DeviceRaw } from '../models/device';
import { DeviceService } from './device.service';
import { TauriService } from './tauri.service';

type Listener = (payload: unknown) => void;

function build(
  invokeImpl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> = async () => {},
) {
  const listeners = new Map<string, Listener[]>();
  const invoke = vi.fn(invokeImpl as never);
  const stubTauri = {
    invoke,
    listen: vi.fn(async (event: string, h: Listener) => {
      listeners.set(event, [...(listeners.get(event) ?? []), h]);
      return () => {};
    }),
  } as unknown as TauriService;
  const injector = Injector.create({
    providers: [
      { provide: TauriService, useValue: stubTauri },
      { provide: DeviceService, useClass: DeviceService },
    ],
  });
  const svc = runInInjectionContext(injector, () => injector.get(DeviceService));
  const ready = (async () => {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  })();
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h(payload);
  };
  return { svc, invoke, ready, emit };
}

const RAW_DEVICE: DeviceRaw = {
  id: 1,
  name: 'DAP',
  kind: 'filesystem',
  device_key: 'fs:/mnt/dap',
  key_is_weak: false,
  root_path: '/Music',
  mount_path: '/mnt/dap',
  last_seen_at: null,
  last_sync_at: null,
  selection: [],
  layout_template: '{title}.{ext}',
  auto_sync: false,
  mirror_deletes: true,
  write_playlist_objects: true,
};

describe('DeviceService', () => {
  it('maps device:progress into the progress signal', async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('device:progress', {
      device_id: 3,
      phase: 'uploading',
      current: 2,
      total: 9,
      message: 'a.flac',
    });
    expect(svc.progress()).toEqual({
      deviceId: 3,
      phase: 'uploading',
      current: 2,
      total: 9,
      message: 'a.flac',
    });
  });

  it('runState reflects running, error and idle transitions', async () => {
    const { svc, ready, emit } = build();
    await ready;
    expect(svc.runState()).toBe('idle');

    emit('device:progress', {
      device_id: 1,
      phase: 'planning',
      current: 0,
      total: 1,
      message: '',
    });
    expect(svc.runState()).toBe('running');

    emit('device:failed', { device_id: 1, error: 'cable' });
    expect(svc.runState()).toBe('error');
  });

  it('accumulates warnings and caps them at 50', async () => {
    const { svc, ready, emit } = build();
    await ready;
    for (let i = 0; i < 60; i += 1) {
      emit('device:warning', {
        device_id: 1,
        kind: 'unsupported_codec',
        detail: `w${i}`,
      });
    }
    const warnings = svc.warnings();
    expect(warnings).toHaveLength(50);
    expect(warnings.at(-1)?.detail).toBe('w59');
  });

  it('device:complete stores the summary and refreshes the device list', async () => {
    const invoke = vi.fn(async (cmd: string) =>
      cmd === 'list_devices' ? [RAW_DEVICE] : undefined,
    );
    const { svc, ready, emit } = build(invoke as never);
    await ready;

    emit('device:complete', {
      device_id: 1,
      added: 3,
      replaced: 1,
      unchanged: 2,
      deleted: 0,
      playlists_written: 1,
      skipped: 0,
      bytes_written: 4096,
    });
    expect(svc.lastComplete()?.playlistsWritten).toBe(1);
    expect(svc.lastComplete()?.bytesWritten).toBe(4096);

    for (let i = 0; i < 10; i += 1) await Promise.resolve();
    expect(invoke).toHaveBeenCalledWith('list_devices');
  });

  it('refresh maps snake_case rows into camelCase devices', async () => {
    const { svc, ready } = build(async (cmd) =>
      cmd === 'list_devices' ? [RAW_DEVICE] : undefined,
    );
    await ready;
    await svc.refresh();
    expect(svc.devices()).toEqual([
      {
        id: 1,
        name: 'DAP',
        kind: 'filesystem',
        deviceKey: 'fs:/mnt/dap',
        keyIsWeak: false,
        rootPath: '/Music',
        mountPath: '/mnt/dap',
        lastSeenAt: null,
        lastSyncAt: null,
        selection: [],
        layoutTemplate: '{title}.{ext}',
        autoSync: false,
        mirrorDeletes: true,
        writePlaylistObjects: true,
      },
    ]);
  });

  it('byId finds a known device and tolerates a null id', async () => {
    const { svc, ready } = build(async (cmd) =>
      cmd === 'list_devices' ? [RAW_DEVICE] : undefined,
    );
    await ready;
    await svc.refresh();
    expect(svc.byId(1)?.name).toBe('DAP');
    expect(svc.byId(99)).toBeUndefined();
    expect(svc.byId(null)).toBeUndefined();
  });

  it('runNow clears prior run state before invoking', async () => {
    const { svc, invoke, ready, emit } = build();
    await ready;
    emit('device:warning', { device_id: 1, kind: 'upload_failed', detail: 'old' });
    emit('device:failed', { device_id: 1, error: 'old' });
    expect(svc.warnings()).toHaveLength(1);

    await svc.runNow(1);
    expect(svc.warnings()).toEqual([]);
    expect(svc.lastError()).toBeNull();
    expect(svc.logLines()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith('run_device_sync', { deviceId: 1 });
  });

  it('runNow records lastError when the command rejects', async () => {
    const { svc, ready } = build(async (cmd) => {
      if (cmd === 'run_device_sync') throw new Error('worker has exited');
      return undefined;
    });
    await ready;
    await svc.runNow(7);
    expect(svc.lastError()).toEqual({ deviceId: 7, error: 'worker has exited' });
  });

  it('preview stores the mapped summary without mutating devices', async () => {
    const { svc, ready } = build(async (cmd) =>
      cmd === 'preview_device_sync'
        ? {
            adds: 3,
            replaces: 1,
            unchanged: 40,
            deletes: 2,
            skips: 1,
            bytes_out: 1500,
            free_bytes: 900,
            total_bytes: 1000,
          }
        : undefined,
    );
    await ready;
    const summary = await svc.preview(1);
    expect(summary.bytesOut).toBe(1500);
    expect(summary.freeBytes).toBe(900);
    expect(svc.lastPlan()).toEqual(summary);
    expect(svc.devices()).toEqual([]);
  });

  it('log lines accumulate in order', async () => {
    const { svc, ready, emit } = build();
    await ready;
    emit('device:log', { device_id: 1, seq: 0, line: 'first' });
    emit('device:log', { device_id: 1, seq: 1, line: 'second' });
    expect(svc.logLines()).toEqual(['first', 'second']);
  });

  it('cancel invokes the backend for that device', async () => {
    const { svc, invoke, ready } = build();
    await ready;
    await svc.cancel(4);
    expect(invoke).toHaveBeenCalledWith('cancel_device_sync', { deviceId: 4 });
  });

  it('pickAndAddDevice returns the new id and refreshes the list', async () => {
    const { svc, invoke, ready } = build(async (cmd) => {
      if (cmd === 'pick_and_add_device') return 11;
      if (cmd === 'list_devices') return [{ ...RAW_DEVICE, id: 11 }];
      return undefined;
    });
    await ready;
    expect(await svc.pickAndAddDevice()).toBe(11);
    expect(invoke).toHaveBeenCalledWith('pick_and_add_device');
    expect(svc.devices().map((d) => d.id)).toEqual([11]);
  });

  it('pickAndAddDevice resolves null when the dialog is dismissed', async () => {
    const { svc, ready } = build(async (cmd) => (cmd === 'pick_and_add_device' ? null : []));
    await ready;
    expect(await svc.pickAndAddDevice()).toBeNull();
  });

  it('ngOnDestroy actually calls every unlisten function', async () => {
    const offs: ReturnType<typeof vi.fn>[] = [];
    const listeners = new Map<string, Listener[]>();
    const stubTauri = {
      invoke: vi.fn(async () => undefined),
      listen: vi.fn(async (event: string, h: Listener) => {
        listeners.set(event, [...(listeners.get(event) ?? []), h]);
        const off = vi.fn();
        offs.push(off);
        return off;
      }),
    } as unknown as TauriService;
    const injector = Injector.create({
      providers: [
        { provide: TauriService, useValue: stubTauri },
        { provide: DeviceService, useClass: DeviceService },
      ],
    });
    const svc = runInInjectionContext(injector, () => injector.get(DeviceService));
    for (let i = 0; i < 20; i += 1) await Promise.resolve();

    expect(offs.length).toBeGreaterThan(0);
    svc.ngOnDestroy();
    for (const off of offs) expect(off).toHaveBeenCalled();
  });
});

describe('formatBytes', () => {
  it('renders byte counts at a readable scale', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1500)).toBe('1.5 kB');
    expect(formatBytes(15_000)).toBe('15 kB');
    expect(formatBytes(1_400_000_000)).toBe('1.4 GB');
  });
});
