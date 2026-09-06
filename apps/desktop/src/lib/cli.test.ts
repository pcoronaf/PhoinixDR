import { describe, expect, it } from "vitest";
import type { ScanRequest } from "../types";
import { explainCommand, recoverCommand, scanCommand, shellQuote } from "./cli";

const base: ScanRequest = {
  source: "C:\\images\\stick.img",
  partition: null,
  volume: null,
  mode: "quick",
  examine_content: true,
  carve: { whole_volume: false, types: [], min_size: 0, alignment: 0 },
};

describe("shellQuote", () => {
  it("leaves plain paths alone and quotes spaces", () => {
    expect(shellQuote("/dev/sdb")).toBe("/dev/sdb");
    expect(shellQuote("\\\\.\\PhysicalDrive1")).toBe("\\\\.\\PhysicalDrive1");
    expect(shellQuote("D:\\My Images\\stick.E01")).toBe('"D:\\My Images\\stick.E01"');
  });
});

describe("scanCommand", () => {
  it("renders a quick scan", () => {
    expect(scanCommand(base)).toBe("phoinix scan C:\\images\\stick.img");
  });
  it("renders partition, deep scan and carving options", () => {
    const r: ScanRequest = {
      ...base,
      partition: 2,
      mode: "deep",
      examine_content: false,
      carve: { whole_volume: true, types: ["jpeg", "pdf"], min_size: 4096, alignment: 4096 },
    };
    expect(scanCommand(r)).toBe(
      "phoinix scan C:\\images\\stick.img --partition 2 --deep --carve-all --carve-types jpeg,pdf --carve-min-size 4096 --carve-align 4096 --no-content",
    );
  });
  it("prefers an explicit volume range over the partition index", () => {
    const r: ScanRequest = { ...base, partition: 1, volume: { offset: 1048576, length: 8388608, repairs: [] } };
    expect(scanCommand(r)).toBe("phoinix scan C:\\images\\stick.img --at 1048576 --length 8388608");
  });
});

describe("explain and recover", () => {
  it("renders the candidate reference and destination", () => {
    expect(explainCommand("/dev/sdb", 1, null, "64")).toBe("phoinix explain /dev/sdb --partition 1 64");
    expect(recoverCommand("/dev/sdb", null, null, ["64", "c1048576"], "/mnt/out dir")).toBe('phoinix recover /dev/sdb 64 c1048576 --output "/mnt/out dir"');
    expect(recoverCommand("/dev/sdb", null, null, ["64"], null)).toBe("phoinix recover /dev/sdb 64 --output <destination>");
  });
});
