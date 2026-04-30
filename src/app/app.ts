import { Component, OnInit, inject } from '@angular/core';
import { LibraryService } from './services/library.service';
import { TransportBarComponent } from './components/transport-bar/transport-bar.component';
import { SidebarComponent } from './components/sidebar/sidebar.component';
import { MainContentComponent } from './components/main-content/main-content.component';

@Component({
  selector: 'app-root',
  imports: [TransportBarComponent, SidebarComponent, MainContentComponent],
  templateUrl: './app.html',
  styleUrl: './app.css',
})
export class App implements OnInit {
  private readonly library = inject(LibraryService);

  ngOnInit(): void {
    void this.library.refreshStats();
  }
}
