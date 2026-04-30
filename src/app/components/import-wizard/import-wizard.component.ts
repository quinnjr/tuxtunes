import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import { ConflictRules, PathMapping, Strategy } from '../../models/sync';
import { SyncService } from '../../services/sync.service';
import { UiService } from '../../services/ui.service';
import { WizardStep } from './wizard-steps';

interface PathRow {
  from: string;
  to: string;
}

@Component({
  selector: 'app-import-wizard',
  imports: [FormsModule],
  templateUrl: './import-wizard.component.html',
})
export class ImportWizardComponent {
  protected readonly sync = inject(SyncService);
  protected readonly open = inject(UiService).importWizardOpen;

  protected readonly step = signal<WizardStep>('pick');
  protected readonly filePath = signal('');
  protected readonly name = signal('iTunes Library');
  protected readonly pathRows = signal<PathRow[]>([
    { from: 'D:/', to: '/run/media/joseph/Local Disk/' },
    { from: 'C:/', to: '/run/media/joseph/Windows/' },
  ]);
  protected readonly autoCopy = signal(false);
  protected readonly rules = signal<ConflictRules>({
    rating: 'prefer_source',
    play_count: 'last_write_wins',
    skip_count: 'last_write_wins',
    last_played: 'last_write_wins',
    last_skipped: 'last_write_wins',
    loved: 'prefer_source',
    deletes: 'respect',
  });

  protected readonly strategyOptions: Strategy[] = [
    'prefer_source',
    'prefer_local',
    'last_write_wins',
  ];

  protected readonly conflictKeys: (keyof ConflictRules)[] = [
    'rating',
    'play_count',
    'skip_count',
    'last_played',
    'last_skipped',
    'loved',
  ];

  hide(): void {
    this.open.set(false);
    this.step.set('pick');
  }

  protected async pickFile(): Promise<void> {
    const picked = await dialogOpen({
      filters: [{ name: 'iTunes Library', extensions: ['itl'] }],
      multiple: false,
    });
    if (typeof picked === 'string') this.filePath.set(picked);
  }

  protected addPathRow(): void {
    this.pathRows.update((r) => [...r, { from: '', to: '' }]);
  }

  protected removePathRow(i: number): void {
    this.pathRows.update((r) => r.filter((_, idx) => idx !== i));
  }

  protected submitFile(): void {
    if (!this.filePath()) return;
    this.step.set('map');
  }

  protected submitMap(): void {
    this.step.set('conflict');
  }

  protected async submitConflict(): Promise<void> {
    const mappings: PathMapping[] = this.pathRows().filter((r) => r.from && r.to);
    const id = await this.sync.addSource({
      name: this.name(),
      source_path: this.filePath(),
      path_mappings: mappings,
      conflict_rules: this.rules(),
      auto_copy_files: this.autoCopy(),
    });
    this.step.set('progress');
    await this.sync.runNow(id);
  }

  protected updateRulesKey(key: keyof ConflictRules, value: string): void {
    this.rules.update((r) => ({ ...r, [key]: value as Strategy }));
  }

  protected updatePathRow(idx: number, field: 'from' | 'to', value: string): void {
    this.pathRows.update((r) => r.map((x, i) => (i === idx ? { ...x, [field]: value } : x)));
  }

  protected onSelectChange(key: keyof ConflictRules, ev: Event): void {
    const target = ev.target as HTMLSelectElement;
    this.updateRulesKey(key, target.value);
  }

  protected toggleAutoCopy(): void {
    this.autoCopy.update((v) => !v);
  }
}
