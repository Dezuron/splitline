import { describe, expect, it } from "vitest";
import { formatBytes, formatTimestamp, versionLabel } from "./labels";

describe("formatBytes", () => {
  it("returns an empty string for zero", () => {
    expect(formatBytes(0)).toBe("");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(2048)).toBe("2 КБ");
  });

  it("formats megabytes", () => {
    expect(formatBytes(2 * 1024 * 1024 + 1)).toBe("2.0 МБ");
  });
});

describe("versionLabel", () => {
  it("keeps a known version", () => {
    expect(versionLabel("1.10.4")).toBe("1.10.4");
  });

  it("reads an empty value as a dash instead of blank", () => {
    expect(versionLabel("")).toBe("—");
    expect(versionLabel("   ")).toBe("—");
    expect(versionLabel(null)).toBe("—");
    expect(versionLabel(undefined)).toBe("—");
  });
});

describe("formatTimestamp", () => {
  it("returns an empty string for zero", () => {
    expect(formatTimestamp(0)).toBe("");
  });

  it("formats a unix timestamp", () => {
    expect(formatTimestamp(1_700_000_000)).toContain("2023");
  });
});
