import { describe, expect, it } from "vitest";
import { parseRange } from "./types";
describe("port entry", () => {
  it("accepts single ports and inclusive ranges", () => {
    expect(parseRange("65535")).toEqual([65535, null]);
    expect(parseRange(" 8000 - 8010 ")).toEqual([8000, 8010]);
  });
  it.each([
    "0",
    "65536",
    "-3",
    "80-79",
    "80-65536",
    "8000-9000",
    "80x",
    "80-90-100",
    "1.5",
    "",
  ])("rejects invalid input %s", (value) => {
    expect(() => parseRange(value)).toThrow();
  });
});
