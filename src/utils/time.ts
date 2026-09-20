/** Formats a UTC SQLite timestamp ("YYYY-MM-DD HH:MM:SS") as a relative age. */
export function formatRelativeTime(value: string | null | undefined): string {
  if (!value) {
    return "never";
  }
  const date = new Date(`${value}Z`);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  const seconds = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000));
  if (seconds < 60) {
    return "just now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes} min ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours} h ago`;
  }
  return date.toLocaleDateString();
}

/** 30 -> "30m", 1440 -> "24h" */
export function formatInterval(minutes: number): string {
  if (minutes % 1440 === 0) {
    return `${minutes / 1440}d`;
  }
  if (minutes % 60 === 0) {
    return `${minutes / 60}h`;
  }
  return `${minutes}m`;
}
