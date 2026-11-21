import { describe, expect, it } from "vitest";
import {
  displayHost,
  isPublicBinding,
  MAX_CF_DOMAINS,
  MAX_DC_IPS,
  MAX_WORKER_DOMAINS,
  normalizeTgwsEngine,
  TGWS_BUF_MAX_KB,
  TGWS_BUF_MIN_KB,
  TGWS_ENGINES,
  TGWS_POOL_MAX,
  TGWS_POOL_MIN,
  tgwsEndpoint,
  tgwsEngineLabel,
  tgwsLimitsError,
  tgwsLogTail,
  tgwsStateLabel,
  tgwsTone,
  tgwsVersionLabel,
} from "./tgws";

describe("tgwsStateLabel", () => {
  it("describes every supervisor state", () => {
    expect(tgwsStateLabel("running")).toBe("Прокси включён");
    expect(tgwsStateLabel("stopped")).toBe("Прокси выключен");
    expect(tgwsStateLabel("not_installed")).toBe("Не установлен");
    expect(tgwsStateLabel(undefined)).toBe("Прокси выключен");
  });
});

describe("tgwsTone", () => {
  it("maps states to status colours", () => {
    expect(tgwsTone("running")).toBe("ok");
    expect(tgwsTone("starting")).toBe("warn");
    expect(tgwsTone("error")).toBe("danger");
    expect(tgwsTone("stopped")).toBe("idle");
  });
});

describe("displayHost", () => {
  it("turns wildcard bindings into loopback", () => {
    expect(displayHost("0.0.0.0")).toBe("127.0.0.1");
    expect(displayHost("::")).toBe("127.0.0.1");
    expect(displayHost("")).toBe("127.0.0.1");
    expect(displayHost("127.0.0.1")).toBe("127.0.0.1");
  });
});

describe("tgwsEndpoint", () => {
  it("renders host:port and brackets IPv6", () => {
    expect(tgwsEndpoint("127.0.0.1", 1443)).toBe("127.0.0.1:1443");
    expect(tgwsEndpoint("0.0.0.0", 1443)).toBe("127.0.0.1:1443");
    expect(tgwsEndpoint("::1", 1443)).toBe("[::1]:1443");
    expect(tgwsEndpoint(undefined, undefined)).toBe("127.0.0.1:1555");
  });
});

describe("isPublicBinding", () => {
  it("flags non-loopback hosts", () => {
    expect(isPublicBinding("127.0.0.1")).toBe(false);
    expect(isPublicBinding("localhost")).toBe(false);
    expect(isPublicBinding("192.168.1.10")).toBe(true);
  });
});

describe("tgwsVersionLabel", () => {
  it("prefers the release tag, then falls back to the binary state", () => {
    expect(tgwsVersionLabel("1.11.2-1", true)).toBe("1.11.2-1");
    expect(tgwsVersionLabel("", true)).toBe("вручную");
    expect(tgwsVersionLabel(null, false)).toBe("не установлен");
    expect(tgwsVersionLabel("  ", undefined)).toBe("не установлен");
  });
});

describe("tgwsLogTail", () => {
  it("keeps only the newest lines", () => {
    const lines = Array.from({ length: 350 }, (_, index) => `line-${index}`);
    const tail = tgwsLogTail(lines);
    expect(tail).toHaveLength(300);
    expect(tail[0]).toBe("line-50");
    expect(tail[299]).toBe("line-349");
    expect(tgwsLogTail(["a"])).toEqual(["a"]);
  });
});

describe("normalizeTgwsEngine", () => {
  it("reads the stored value leniently, like the backend", () => {
    expect(normalizeTgwsEngine("rs")).toBe("rs");
    expect(normalizeTgwsEngine(" RS ")).toBe("rs");
    expect(normalizeTgwsEngine("python")).toBe("python");
    // `rust` is the backend's alias for the same engine.
    expect(normalizeTgwsEngine("rust")).toBe("rs");
    // An unknown or missing value is the engine the app would actually run.
    expect(normalizeTgwsEngine("java")).toBe("python");
    expect(normalizeTgwsEngine("")).toBe("python");
    expect(normalizeTgwsEngine(undefined)).toBe("python");
    expect(normalizeTgwsEngine(null)).toBe("python");
  });
});

describe("tgwsEngineLabel", () => {
  it("names both engines for the diagnostics card", () => {
    expect(tgwsEngineLabel("rs")).toBe("Новый (beta)");
    expect(tgwsEngineLabel("python")).toBe("Старый");
    expect(tgwsEngineLabel(undefined)).toBe("Старый");
  });
});

describe("TGWS_ENGINES", () => {
  it("offers both engines, old first, and marks the new one as beta", () => {
    expect(TGWS_ENGINES.map((option) => option.value)).toEqual(["python", "rs"]);
    expect(TGWS_ENGINES[0].label).toContain("проверенный");
    expect(TGWS_ENGINES[1].label).toContain("beta");
    // The page and the backend must cap the list the same way.
    expect(MAX_CF_DOMAINS).toBe(10);
  });
});

describe("list caps", () => {
  it("matches the backend limits for Worker domains and DC redirects", () => {
    expect(MAX_WORKER_DOMAINS).toBe(10);
    expect(MAX_DC_IPS).toBe(10);
  });
});

describe("tgwsLimitsError", () => {
  it("accepts the whole allowed range", () => {
    expect(tgwsLimitsError(String(TGWS_BUF_MIN_KB), String(TGWS_POOL_MIN))).toBe("");
    expect(tgwsLimitsError(String(TGWS_BUF_MAX_KB), String(TGWS_POOL_MAX))).toBe("");
    expect(tgwsLimitsError("512", "4")).toBe("");
    expect(tgwsLimitsError(" 512 ", " 4 ")).toBe("");
  });

  it("explains a bad buffer before touching the pool", () => {
    expect(tgwsLimitsError("3", "4")).toContain("Буфер");
    expect(tgwsLimitsError("4097", "4")).toContain("Буфер");
    expect(tgwsLimitsError("", "4")).toContain("Буфер");
    expect(tgwsLimitsError("4.5", "4")).toContain("Буфер");
    expect(tgwsLimitsError("-1", "4")).toContain("Буфер");
  });

  it("explains a bad pool", () => {
    expect(tgwsLimitsError("256", "33")).toContain("Пул");
    expect(tgwsLimitsError("256", "-1")).toContain("Пул");
    expect(tgwsLimitsError("256", "")).toContain("Пул");
    expect(tgwsLimitsError("256", "много")).toContain("Пул");
  });
});
