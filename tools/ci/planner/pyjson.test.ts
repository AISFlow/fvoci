import { describe, expect, test } from "bun:test";
import { PyFloat, PyInt, pyDumps, pyFloatRepr, pyLoads, pyQuote, pySorted } from "./pyjson.ts";

// Expected strings are what CPython's json.dumps / repr(float) print.
describe("python-compatible json", () => {
  test("float repr", () => {
    const cases: [number, string][] = [
      [1, "1.0"],
      [-0, "-0.0"],
      [0, "0.0"],
      [1.5, "1.5"],
      [1e16, "1e+16"],
      [1e15, "1000000000000000.0"],
      [123456789.125, "123456789.125"],
      [0.0001, "0.0001"],
      [0.00001, "1e-05"],
      [1.5e-7, "1.5e-07"],
      [1.7976931348623157e308, "1.7976931348623157e+308"],
      [5e-324, "5e-324"],
      [0.1, "0.1"],
    ];
    for (const [value, repr] of cases) expect(pyFloatRepr(value), repr).toBe(repr);
    for (const value of [Infinity, -Infinity, NaN]) expect(() => pyFloatRepr(value)).toThrow();
  });

  test("strings escape every non-ASCII code unit", () => {
    expect(pyQuote('a"\\\n\r\t\b\f\u0001\u007f é \u2028 \u{1F600}')).toBe(
      '"a\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u007f \\u00e9 \\u2028 \\ud83d\\ude00"',
    );
    expect(pyQuote("/")).toBe('"/"');
  });

  test("loads keeps source order, integer digits and the last duplicate", () => {
    const value = pyLoads('{"2": 1, "1": [1.0, -0, 12345678901234567890], "2": 2.5}');
    expect(pyDumps(value)).toBe('{"2": 2.5, "1": [1.0, 0, 12345678901234567890]}');
    expect(pyDumps(value, { compact: true, sortKeys: true })).toBe(
      '{"1":[1.0,0,12345678901234567890],"2":2.5}',
    );
    expect(pyLoads(" 7 ")).toEqual(new PyInt(7n));
    expect(pyLoads("1e2")).toEqual(new PyFloat(100));
    expect(pyLoads('"\\ud800"')).toBe("\ud800");
  });

  test("loads is strict JSON", () => {
    for (const bad of [
      "",
      "{",
      "[1,]",
      "01",
      "+1",
      "1.",
      ".5",
      "'x'",
      '"a\nb"',
      "nul",
      "{} x",
      '{"a" 1}',
      '"\\x"',
      "1".repeat(4301),
      // Python's json accepts these and would write invalid JSON back.
      "NaN",
      "Infinity",
      "-Infinity",
      "1e400",
      "\u{feff}{}",
    ]) {
      expect(() => pyLoads(bad), bad).toThrow();
    }
  });

  test("dumps layouts", () => {
    const plan = { a: 1, b: { y: true, x: null }, c: [], d: {} };
    expect(pyDumps(plan)).toBe('{"a": 1, "b": {"y": true, "x": null}, "c": [], "d": {}}');
    expect(pyDumps(plan, { indent: 2 })).toBe(
      '{\n  "a": 1,\n  "b": {\n    "y": true,\n    "x": null\n  },\n  "c": [],\n  "d": {}\n}',
    );
    expect(pyDumps(plan, { compact: true, sortKeys: true })).toBe(
      '{"a":1,"b":{"x":null,"y":true},"c":[],"d":{}}',
    );
  });

  test("sorted() order is by code point", () => {
    expect(pySorted(["\u{1F600}", "\uffff", "b", "B"])).toEqual(["B", "b", "\uffff", "\u{1F600}"]);
  });
});
