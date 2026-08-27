import { Injectable, inject } from '@angular/core';
import { FaIconLibrary } from '@fortawesome/angular-fontawesome';
import {
  faBackwardStep,
  faFileImport,
  faForwardStep,
  faGear,
  faMusic,
  faPause,
  faPlay,
  faPlus,
  faRepeat,
  faShuffle,
  faStop,
  faVolumeUp,
} from '@fortawesome/free-solid-svg-icons';

@Injectable({ providedIn: 'root' })
export class IconRegistry {
  private readonly library = inject(FaIconLibrary);

  constructor() {
    this.library.addIcons(
      faBackwardStep,
      faFileImport,
      faForwardStep,
      faGear,
      faMusic,
      faPause,
      faPlay,
      faPlus,
      faRepeat,
      faShuffle,
      faStop,
      faVolumeUp,
    );
  }
}
