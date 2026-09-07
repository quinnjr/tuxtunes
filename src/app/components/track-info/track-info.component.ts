import {
  Component,
  HostListener,
  computed,
  effect,
  inject,
  signal,
  ChangeDetectionStrategy,
} from '@angular/core';
import { LibraryService, TrackMetadataEdit } from '../../services/library.service';
import { UiService } from '../../services/ui.service';

interface FormState {
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  genre: string;
  year: string;
  trackNumber: string;
  discNumber: string;
}

const EMPTY_FORM: FormState = {
  title: '',
  artist: '',
  album: '',
  albumArtist: '',
  genre: '',
  year: '',
  trackNumber: '',
  discNumber: '',
};

/**
 * "Get Info…" editor for a track's descriptive metadata. Driven by
 * `ui.trackInfo`. Saving writes the file's tags and the DB through the
 * backend; failures keep the dialog open so nothing typed is lost.
 */
@Component({
  selector: 'app-track-info',
  imports: [],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './track-info.component.html',
})
export class TrackInfoComponent {
  protected readonly ui = inject(UiService);
  protected readonly library = inject(LibraryService);

  protected readonly form = signal<FormState>({ ...EMPTY_FORM });
  protected readonly saving = signal(false);

  protected readonly row = computed(this.#computeRow.bind(this));

  #computeRow() {
    const req = this.ui.trackInfo();
    if (req === null) return null;
    return this.library.tracksById().get(req.trackId) ?? null;
  }

  constructor() {
    // Re-seed the form each time a (different) track is opened.
    effect(() => {
      const row = this.row();
      this.form.set(
        row === null
          ? { ...EMPTY_FORM }
          : {
              title: row.title,
              artist: row.artist ?? '',
              album: row.album ?? '',
              albumArtist: row.albumArtist ?? '',
              genre: row.genre ?? '',
              year: row.year === null ? '' : String(row.year),
              trackNumber: row.trackNumber === null ? '' : String(row.trackNumber),
              discNumber: row.discNumber === null ? '' : String(row.discNumber),
            },
      );
    });
  }

  @HostListener('document:keydown.escape')
  onEscape(): void {
    if (this.ui.trackInfo() !== null) this.cancel();
  }

  /** Keep keystrokes away from global single-key shortcuts. */
  protected onKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Escape') event.stopPropagation();
  }

  protected onInput(field: keyof FormState, event: Event): void {
    const value = (event.target as HTMLInputElement).value;
    this.form.update((f) => ({ ...f, [field]: value }));
  }

  protected canSave(): boolean {
    return this.form().title.trim().length > 0 && !this.saving();
  }

  protected async save(event: Event): Promise<void> {
    event.preventDefault();
    const req = this.ui.trackInfo();
    if (req === null || !this.canSave()) return;
    const f = this.form();
    const edit: TrackMetadataEdit = {
      title: f.title.trim(),
      artist: optText(f.artist),
      album: optText(f.album),
      albumArtist: optText(f.albumArtist),
      genre: optText(f.genre),
      year: optInt(f.year),
      trackNumber: optInt(f.trackNumber),
      discNumber: optInt(f.discNumber),
    };
    this.saving.set(true);
    try {
      await this.library.updateTrackMetadata(req.trackId, edit);
      this.ui.trackInfo.set(null);
    } catch (error) {
      // Keep the dialog (and the user's typing) alive on failure.
      this.ui.reportError(error);
    } finally {
      this.saving.set(false);
    }
  }

  protected cancel(): void {
    this.ui.trackInfo.set(null);
  }
}

function optText(value: string): string | null {
  const trimmed = value.trim();
  return trimmed.length === 0 ? null : trimmed;
}

function optInt(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed.length === 0) return null;
  const n = Number.parseInt(trimmed, 10);
  return Number.isNaN(n) ? null : n;
}
