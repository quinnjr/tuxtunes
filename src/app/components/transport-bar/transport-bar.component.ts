import { Component } from '@angular/core';
import { FaIconComponent } from '@fortawesome/angular-fontawesome';
import {
  faBackwardStep,
  faForwardStep,
  faPlay,
  faRepeat,
  faShuffle,
} from '@fortawesome/free-solid-svg-icons';

@Component({
  selector: 'app-transport-bar',
  imports: [FaIconComponent],
  templateUrl: './transport-bar.component.html',
})
export class TransportBarComponent {
  protected readonly faPlay = faPlay;
  protected readonly faPrev = faBackwardStep;
  protected readonly faNext = faForwardStep;
  protected readonly faShuffle = faShuffle;
  protected readonly faRepeat = faRepeat;
}
