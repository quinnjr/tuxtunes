import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import type { ConflictRules } from '../../models/sync';
import { SyncService } from '../../services/sync.service';
import { UiService } from '../../services/ui.service';
import { appProviders, tauriStub } from '../../test-helpers';
import { ImportWizardComponent } from './import-wizard.component';
import type { WizardStep } from './wizard-steps';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

import { open as dialogOpen } from '@tauri-apps/plugin-dialog';

interface WizardInternals {
  step: { (): WizardStep; set(v: WizardStep): void };
  filePath: { (): string; set(v: string): void };
  name: { (): string; set(v: string): void };
  pathRows: { (): { from: string; to: string }[] };
  autoCopy: { (): boolean };
  rules: { (): ConflictRules };
  open: { (): boolean; set(v: boolean): void };
  hide(): void;
  pickFile(): Promise<void>;
  addPathRow(): void;
  removePathRow(i: number): void;
  submitFile(): void;
  submitMap(): void;
  submitConflict(): Promise<void>;
  updateRulesKey(key: keyof ConflictRules, value: string): void;
  updatePathRow(idx: number, field: 'from' | 'to', value: string): void;
  onSelectChange(key: keyof ConflictRules, ev: Event): void;
  toggleAutoCopy(): void;
}

function setup() {
  const stub = tauriStub();
  TestBed.configureTestingModule({
    imports: [ImportWizardComponent],
    providers: appProviders(stub),
  });
  const fixture = TestBed.createComponent(ImportWizardComponent);
  fixture.detectChanges();
  return {
    fixture,
    cmp: fixture.componentInstance as unknown as WizardInternals,
    sync: TestBed.inject(SyncService),
    ui: TestBed.inject(UiService),
  };
}

describe('ImportWizardComponent', () => {
  it('hide() resets the wizard step and closes the panel', () => {
    const { cmp, ui } = setup();
    ui.importWizardOpen.set(true);
    cmp.step.set('progress');
    cmp.hide();
    expect(ui.importWizardOpen()).toBe(false);
    expect(cmp.step()).toBe('pick');
  });

  it('pickFile stores a string return; ignores cancellations', async () => {
    const { cmp } = setup();
    (dialogOpen as ReturnType<typeof vi.fn>).mockResolvedValueOnce('/itunes.itl');
    await cmp.pickFile();
    expect(cmp.filePath()).toBe('/itunes.itl');
    cmp.filePath.set('/before');
    (dialogOpen as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    await cmp.pickFile();
    expect(cmp.filePath()).toBe('/before');
  });

  it('addPathRow / removePathRow / updatePathRow mutate the row signal', () => {
    const { cmp } = setup();
    const baseLen = cmp.pathRows().length;
    cmp.addPathRow();
    expect(cmp.pathRows().length).toBe(baseLen + 1);
    cmp.updatePathRow(baseLen, 'from', 'X:');
    cmp.updatePathRow(baseLen, 'to', '/x');
    expect(cmp.pathRows()[baseLen]).toEqual({ from: 'X:', to: '/x' });
    cmp.removePathRow(baseLen);
    expect(cmp.pathRows().length).toBe(baseLen);
  });

  it('submitFile rejects an empty path and advances to map otherwise', () => {
    const { cmp } = setup();
    cmp.submitFile();
    expect(cmp.step()).toBe('pick');
    cmp.filePath.set('/x.itl');
    cmp.submitFile();
    expect(cmp.step()).toBe('map');
  });

  it('submitMap advances to the conflict step', () => {
    const { cmp } = setup();
    cmp.submitMap();
    expect(cmp.step()).toBe('conflict');
  });

  it('submitConflict filters incomplete path rows and runs the sync', async () => {
    const { cmp, sync } = setup();
    cmp.filePath.set('/x.itl');
    // Inject one full row + one half-empty row.
    cmp.updatePathRow(0, 'from', 'D:/');
    cmp.updatePathRow(0, 'to', '/mnt/d');
    cmp.updatePathRow(1, 'from', 'C:/');
    cmp.updatePathRow(1, 'to', '');
    const addSpy = vi.spyOn(sync, 'addSource').mockResolvedValue(7);
    const runSpy = vi.spyOn(sync, 'runNow').mockResolvedValue();
    await cmp.submitConflict();
    expect(addSpy).toHaveBeenCalled();
    const args = addSpy.mock.calls[0][0];
    expect(args.path_mappings).toEqual([{ from: 'D:/', to: '/mnt/d' }]);
    expect(runSpy).toHaveBeenCalledWith(7);
    expect(cmp.step()).toBe('progress');
  });

  it('updateRulesKey + onSelectChange write strategy values', () => {
    const { cmp } = setup();
    cmp.updateRulesKey('rating', 'prefer_local');
    expect(cmp.rules().rating).toBe('prefer_local');
    cmp.onSelectChange('play_count', {
      target: { value: 'prefer_source' },
    } as unknown as Event);
    expect(cmp.rules().play_count).toBe('prefer_source');
  });

  it('toggleAutoCopy flips the boolean', () => {
    const { cmp } = setup();
    expect(cmp.autoCopy()).toBe(false);
    cmp.toggleAutoCopy();
    expect(cmp.autoCopy()).toBe(true);
  });
});
