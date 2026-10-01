import { test, expect, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { installAppFixture } from "./fixtures/appFixture";

/**
 * Computed-style regression for the move off hand-written CSS.
 *
 * Keyed by class name rather than by position in the tree: the question this
 * has to answer is "does `.server-item` still compute the same styles now that
 * its rules are utilities?", and that must hold regardless of how many rows
 * the fixture happens to render. Geometry is deliberately excluded — panel
 * content varies with load timing, and layout is already covered by the
 * behavioural suite in app.spec.ts.
 *
 * Refresh deliberately, never casually:
 *   UPDATE_STYLE_SNAPSHOT=1 npx playwright test tests/styles.spec.ts --project=chromium
 */
const PANELS = [
  "Overview",
  "Connections",
  "Services",
  "Containers",
  "Commands",
  "Integration",
] as const;

const PROPS = [
  "display",
  "position",
  "color",
  "backgroundColor",
  "fontSize",
  "fontWeight",
  "lineHeight",
  "letterSpacing",
  "textTransform",
  "textAlign",
  "paddingTop",
  "paddingRight",
  "paddingBottom",
  "paddingLeft",
  "marginTop",
  "marginRight",
  "marginBottom",
  "marginLeft",
  "borderTopWidth",
  "borderRightWidth",
  "borderBottomWidth",
  "borderLeftWidth",
  "borderTopColor",
  "borderRadius",
  "gridTemplateColumns",
  "gap",
  "flexDirection",
  "flexWrap",
  "justifyContent",
  "alignItems",
  "opacity",
  "overflow",
  "whiteSpace",
  "boxShadow",
  "minHeight",
  "maxWidth",
] as const;

const here = path.dirname(fileURLToPath(import.meta.url));
/*
 * One baseline per browser. Chromium and WebKit genuinely compute some of these
 * properties differently -- `minHeight: 0px` against `auto`, `overflow: clip`
 * against `visible`, grid tracks that differ in the third decimal -- so a single
 * recorded file can only ever pass in the browser that recorded it. Rounding
 * (see captureByClass) removes the serialisation noise; these are the real
 * remainder, and they need a baseline each.
 */
const snapshotFor = (project: string) =>
  path.join(here, "__snapshots__", `styles.${project}.json`);
const UPDATE = process.env.UPDATE_STYLE_SNAPSHOT === "1";
// These resolved values measure content/font geometry, not CSS regressions:
// auto margins, an implicit grid track, and 70ch vary with OS font metrics.
const CONTENT_GEOMETRY: Record<string, string> = {
  "source-count": "marginLeft",
  "overview-refresh": "gridTemplateColumns",
  "section-description": "maxWidth",
};

/** First element bearing each class, and what the browser computes for it. */
async function captureByClass(page: Page) {
  return page.evaluate(
    (props) => {
      const out: Record<string, string> = {};
      for (const el of document.querySelectorAll<HTMLElement>("[class]")) {
        const cs = getComputedStyle(el);
        for (const name of el.classList) {
          if (out[name]) continue;
          // Browsers serialise computed numbers to different precision --
          // WebKit reports `color(srgb 0.098039 …)` and `18.571428px` where
          // Chromium reports `0.0980392` and `18.5714px`. Comparing the raw
          // strings therefore reported 167 "changes" that were nothing but
          // formatting, on a baseline that can only ever be recorded in one
          // browser. Rounding leaves real differences intact.
          out[name] = props
            .map((p) =>
              String(cs[p as never] as unknown as string).replace(
                /-?\d+\.\d+/g,
                (n) => String(Math.round(parseFloat(n) * 1000) / 1000),
              ),
            )
            .join("|");
        }
      }
      return out;
    },
    PROPS as unknown as string[],
  );
}

test("class styles match the recorded baseline", async ({ page }) => {
  test.setTimeout(120_000);
  // Relative timestamps and uptime readouts otherwise change between runs.
  await page.clock.setFixedTime(new Date("2026-01-01T12:00:00Z"));
  await installAppFixture(page);
  await page.goto("/");

  const captured: Record<string, string> = {};
  for (const panel of PANELS) {
    await page.getByRole("tab", { name: panel, exact: true }).click();
    await page.waitForTimeout(300);
    // A class first seen on one panel keeps that panel's computed value.
    for (const [name, value] of Object.entries(await captureByClass(page))) {
      if (!(name in captured)) captured[name] = value;
    }
  }

  const snapshot = snapshotFor(test.info().project.name);
  if (UPDATE || !fs.existsSync(snapshot)) {
    fs.mkdirSync(path.dirname(snapshot), { recursive: true });
    fs.writeFileSync(snapshot, `${JSON.stringify(captured, null, 1)}\n`);
    test.info().annotations.push({
      type: "snapshot",
      description: `recorded ${Object.keys(captured).length} classes`,
    });
    return;
  }

  const baseline: Record<string, string> = JSON.parse(
    fs.readFileSync(snapshot, "utf8"),
  );
  const drift: string[] = [];
  for (const [name, before] of Object.entries(baseline)) {
    // xterm owns these generated styles and adjusts them to measured cell
    // dimensions. Its rendering is covered by terminal tests, not this app-CSS baseline.
    if (name.startsWith("xterm") || name === "live-region") continue;
    const now = captured[name];
    // A class that has gone is expected while converting to utilities; what
    // matters is that the classes still in use look the same.
    if (now === undefined || now === before) continue;
    const a = before.split("|");
    const b = now.split("|");
    const changed = PROPS.map((p, i) =>
      a[i] === b[i] || CONTENT_GEOMETRY[name] === p
        ? null
        : `${p}: ${a[i]} -> ${b[i]}`,
    ).filter(Boolean);
    if (changed.length) drift.push(`.${name} {${changed.join("; ")}}`);
  }

  expect(drift, `${drift.length} class(es) changed appearance`).toEqual([]);
});
