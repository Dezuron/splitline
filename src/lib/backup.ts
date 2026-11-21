/**
 * Name of the settings archive the save dialog suggests:
 * `splitline-backup-YYYYMMDD-HHMM.zip` with local date and time, so several
 * backups of one day sort in the order they were made.
 */
export function backupFileName(now: Date = new Date()): string {
  return `splitline-backup-${backupStamp(now)}.zip`;
}

/** `YYYYMMDD-HHMM` in local time; also used by the tests of the archive format. */
export function backupStamp(now: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return [
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`,
    `${pad(now.getHours())}${pad(now.getMinutes())}`,
  ].join("-");
}

/** Last path component, for a toast: the full path does not fit one line. */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
