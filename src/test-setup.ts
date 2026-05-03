// Pull Angular's JIT compiler into the test bundle so `@Injectable`
// classes get their factory generated when an Injector tries to
// instantiate them. Without this, ng_factory_def calls into a
// no-op compiler facade and throws.
import '@angular/compiler';
