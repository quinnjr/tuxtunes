import { Directive, ElementRef, OnDestroy, OnInit, inject, output } from '@angular/core';

/**
 * Emits `appInView` once, the first time the host element scrolls into
 * the viewport (with a small margin so the row below the fold starts
 * loading early). Used to defer per-card work — artwork lookups — to
 * cards the user can actually see, instead of hammering the backend
 * for every album in a 3,000-album grid on mount.
 *
 * Environments without IntersectionObserver (unit tests) emit
 * immediately on init.
 */
@Directive({ selector: '[appInView]' })
export class InViewDirective implements OnInit, OnDestroy {
  readonly appInView = output<void>();

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private observer: IntersectionObserver | null = null;
  private fired = false;

  ngOnInit(): void {
    if (typeof IntersectionObserver === 'undefined') {
      this.appInView.emit();
      return;
    }
    this.observer = new IntersectionObserver(
      (entries) => {
        if (this.fired || !entries.some((e) => e.isIntersecting)) return;
        this.fired = true;
        this.appInView.emit();
        this.disconnect();
      },
      { rootMargin: '200px 0px' },
    );
    this.observer.observe(this.host.nativeElement);
  }

  ngOnDestroy(): void {
    this.disconnect();
  }

  private disconnect(): void {
    this.observer?.disconnect();
    this.observer = null;
  }
}
