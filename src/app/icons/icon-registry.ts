import { Injectable, inject } from '@angular/core';
import { FaIconLibrary } from '@fortawesome/angular-fontawesome';
import { faMusic, faPlay } from '@fortawesome/free-solid-svg-icons';

@Injectable({ providedIn: 'root' })
export class IconRegistry {
  private readonly library = inject(FaIconLibrary);

  constructor() {
    this.library.addIcons(faMusic, faPlay);
  }
}
