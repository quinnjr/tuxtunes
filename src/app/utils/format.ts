/** Format a millisecond total duration as e.g. "3 days, 7:23:45". */
export function formatTotalDuration(ms: number): string {
  if (ms <= 0) return '0:00';
  const totalSec = Math.round(ms / 1000);
  const days = Math.floor(totalSec / 86_400);
  const rem = totalSec % 86_400;
  const h = Math.floor(rem / 3600);
  const m = Math.floor((rem % 3600) / 60);
  const s = rem % 60;
  const hms = `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  if (days === 0) return hms;
  return `${days} ${days === 1 ? 'day' : 'days'}, ${hms}`;
}

/** Format a byte count as a binary-prefixed size string (KiB/MiB/GiB/TiB). */
export function formatByteSize(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  const fixed = i === 0 ? value.toFixed(0) : value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2);
  return `${fixed} ${units[i]}`;
}
