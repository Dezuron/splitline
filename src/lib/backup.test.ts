import { describe, expect, it } from "vitest";
import { backupFileName, backupStamp, fileNameOf } from "./backup";

describe("backupFileName", () => {
  it("uses the documented local date-time format", () => {
    expect(backupFileName(new Date(2026, 10, 20, 18, 5))).toBe(
      "splitline-backup-20261120-1805.zip",
    );
  });

  it("pads month, day, hour and minute", () => {
    expect(backupStamp(new Date(2026, 0, 3, 4, 7))).toBe("20260103-0407");
  });
});

describe("fileNameOf", () => {
  it("returns the last component of a Windows path", () => {
    expect(fileNameOf("C:\\Users\\user\\backups\\backup-20261120-1805.zip")).toBe(
      "backup-20261120-1805.zip",
    );
  });

  it("keeps a bare name as is", () => {
    expect(fileNameOf("backup.zip")).toBe("backup.zip");
  });
});
