/** Bytes-per-second as a compact human string ("812.5 KiB/s"). */
export const formatRate = (bytesPerSecond: number): string => {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) {
    return "0 B/s";
  }
  const units = ["B/s", "KiB/s", "MiB/s", "GiB/s"];
  let value = bytesPerSecond;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  const rounded = Number(value.toFixed(digits));
  return `${rounded} ${units[unit]}`;
};
