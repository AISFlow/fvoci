import { describe, expect, test } from "bun:test";
import {
  PyFloat,
  PyInt,
  pyDumps,
  pyFloatRepr,
  pyLoads,
  pyQuote,
  pySorted,
  pyTruthy,
} from "./pyjson.ts";

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
      [Infinity, "Infinity"],
      [-Infinity, "-Infinity"],
      [NaN, "NaN"],
    ];
    for (const [value, repr] of cases) expect(pyFloatRepr(value), repr).toBe(repr);
  });

  test("strings escape every non-ASCII code unit", () => {
    expect(pyQuote('a"\\\n\r\t\b\f\u0001\u007f é \u2028 \u{1F600}')).toBe(
      '"a\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u007f \\u00e9 \\u2028 \\ud83d\\ude00"',
    );
    expect(pyQuote("/")).toBe('"/"');
  });

  test("loads keeps source order, integer digits and the last duplicate", () => {
    const value = pyLoads(
      '{"2": 1, "1": [1.0, -0, 12345678901234567890], "2": NaN, "x": -Infinity}',
    );
    expect(pyDumps(value)).toBe('{"2": NaN, "1": [1.0, 0, 12345678901234567890], "x": -Infinity}');
    expect(pyDumps(value, { compact: true, sortKeys: true })).toBe(
      '{"1":[1.0,0,12345678901234567890],"2":NaN,"x":-Infinity}',
    );
    expect(pyLoads("1e400")).toEqual(new PyFloat(Infinity));
    expect(pyLoads(" 7 ")).toEqual(new PyInt(7n));
    expect(pyLoads('"\\ud800"')).toBe("\ud800");
  });

  test("loads refuses what CPython refuses", () => {
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

  test("sorted() order and truthiness", () => {
    expect(pySorted(["\u{1F600}", "\uffff", "b", "B"])).toEqual(["B", "b", "\uffff", "\u{1F600}"]);
    for (const falsy of ["0", "0.0", "-0.0", '""', "[]", "{}", "null", "false"])
      expect(pyTruthy(pyLoads(falsy)), falsy).toBe(false);
    for (const truthy of ["1", "NaN", '"0"', "[0]", '{"a":0}', "true"])
      expect(pyTruthy(pyLoads(truthy)), truthy).toBe(true);
  });
});
