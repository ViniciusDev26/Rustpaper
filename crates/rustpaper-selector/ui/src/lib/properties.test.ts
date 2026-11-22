import { describe, expect, it } from "vitest";
import { colorHint, evalCondition, fromHex, isHex, label, plainText, toHex } from "./properties";

describe("conditions", () => {
  const v = { mode: "2", clock: true, size: 5, name: "x" };
  it("compares loosely like WE", () => {
    expect(evalCondition("mode.value == 2", v)).toBe(true);
    expect(evalCondition("mode == '2'", v)).toBe(true);
    expect(evalCondition("mode.value != 2", v)).toBe(false);
  });
  it("combines with precedence and parentheses", () => {
    expect(evalCondition("clock && size > 3", v)).toBe(true);
    expect(evalCondition("!clock || size >= 6", v)).toBe(false);
    expect(evalCondition("(clock || false) && !(size < 5)", v)).toBe(true);
  });
  it("shows the control when unparseable", () => {
    expect(evalCondition("clock &&", v)).toBe(true);
    expect(evalCondition("alert(1)", v)).toBe(true);
  });
  it("never evaluates code", () => {
    // a function-call shape is a parse error, not a call
    expect(evalCondition("constructor.constructor('x')()", v)).toBe(true);
  });
});

describe("labels and text", () => {
  it("maps WE localization keys", () => {
    expect(label({ name: "a", text: "ui_browse_properties_scheme_color" })).toBe("Scheme color");
    expect(label({ name: "a", text: "ui_my_option" })).toBe("My option");
  });
  it("strips author HTML", () => {
    expect(plainText("<b>Bold</b><br/>next &amp; more")).toBe("Bold next & more");
    expect(label({ name: "fallback", text: "<i></i>" })).toBe("fallback");
  });
});

describe("colors", () => {
  it("round-trips WE's 0..1 floats", () => {
    expect(toHex("1 0.5 0")).toBe("#ff8000");
    expect(fromHex("#ff8000")).toBe("1.00000 0.50196 0.00000");
    expect(toHex("255 0 0")).toBe("#ff0000");
    expect(toHex("garbage")).toBe("#ffffff");
  });
});

describe("color helpers", () => {
  it("recognizes hex codes", () => {
    expect(isHex("#1a2B3c")).toBe(true);
    expect(isHex("1a2b3c")).toBe(false);
    expect(isHex("#12345")).toBe(false);
  });
  it("explains the scheme color only", () => {
    expect(colorHint({ name: "schemecolor" })).toMatch(/theme color/);
    expect(colorHint({ name: "x", text: "ui_browse_properties_scheme_color" })).toMatch(/theme color/);
    expect(colorHint({ name: "barcolor", text: "Bar Color" })).toBeNull();
  });
  it("round-trips WE colors through hex", () => {
    expect(toHex(fromHex("#ff8000"))).toBe("#ff8000");
  });
});
