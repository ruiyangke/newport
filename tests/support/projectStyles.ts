import { expect, type Page, type TestInfo } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Computed-style regression for the Projects surfaces.
 *
 * The Projects stylesheet is being split per component and then moved onto
 * Tailwind utilities. Both steps are supposed to leave what the browser
 * computes untouched, and neither can be judged by eye: the split reorders
 * rules in the bundle, which silently flips any conflict between two equally
 * specific rules, and a utility that loses to an unlayered rule looks applied
 * in the source while computing nothing. This records, for every bespoke
 * `git-*` / `projects-*` class the flow reaches, what its first element
 * computes, and fails on any change.
 *
 * Refresh deliberately, never casually, and only for a change that was meant:
 *   UPDATE_PROJECT_STYLES=1 npx playwright test tests/projects.spec.ts
 */
const PROPS = [
  "display",
  "position",
  "color",
  "backgroundColor",
  "fontSize",
  "fontWeight",
  "lineHeight",
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
  "flexGrow",
  "flexShrink",
  "justifyContent",
  "alignItems",
  "opacity",
  "overflow",
  "whiteSpace",
  "textOverflow",
  "boxShadow",
  "minHeight",
  "minWidth",
  "maxWidth",
] as const;

const here = path.dirname(fileURLToPath(import.meta.url));
const snapshotFor = (project: string) =>
  path.join(here, "..", "__snapshots__", `projects-styles.${project}.json`);

/** The first element bearing each bespoke class, and what it computes. */
export async function captureProjectStyles(page: Page) {
  return page.evaluate(
    async (props) => {
      // A read taken mid-animation caught a dialog mid-fade at a different
      // opacity on every run. Finite animations are waited out, not finished:
      // finishing one completed an exit early in WebKit and let the flow run
      // ahead of an element it was about to check. Waiting only delays; it
      // changes nothing the test goes on to look at. Infinite ones (spinners)
      // are not a class's resting style and are not waited for.
      const finite = document.getAnimations().filter((animation) => {
        const end = animation.effect?.getComputedTiming().endTime;
        return typeof end === "number" && Number.isFinite(end);
      });
      await Promise.race([
        Promise.all(finite.map((each) => each.finished.catch(() => undefined))),
        new Promise((resolve) => setTimeout(resolve, 2000)),
      ]);
      const out: Record<string, string> = {};
      for (const el of document.querySelectorAll<HTMLElement>("[class]")) {
        if (typeof el.className !== "string") continue;
        const cs = getComputedStyle(el);
        for (const name of el.classList) {
          if (!/^(git|projects)-/.test(name) || out[name]) continue;
          // Browsers serialise computed numbers to different precisions, so
          // rounding keeps the comparison about values rather than formatting.
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

/**
 * Captures at every screenshot the flow already takes: those are the states
 * someone decided were worth looking at, so they are the states worth pinning.
 * A class keeps the value from the first state it appeared in.
 */
export function recordProjectStyles(page: Page) {
  const captured: Record<string, string> = {};
  const shoot = page.screenshot.bind(page);
  page.screenshot = async (options) => {
    for (const [name, value] of Object.entries(
      await captureProjectStyles(page),
    ))
      if (!(name in captured)) captured[name] = value;
    return shoot(options);
  };
  return captured;
}

export function expectProjectStyles(
  captured: Record<string, string>,
  info: TestInfo,
) {
  const snapshot = snapshotFor(info.project.name);
  if (process.env.UPDATE_PROJECT_STYLES === "1" || !fs.existsSync(snapshot)) {
    fs.mkdirSync(path.dirname(snapshot), { recursive: true });
    fs.writeFileSync(snapshot, `${JSON.stringify(captured, null, 2)}\n`);
    info.annotations.push({
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
    const now = captured[name];
    // A class that has gone is expected while its rules become utilities on
    // the element; what matters is that the classes still present compute the
    // same.
    if (now === undefined || now === before) continue;
    const a = before.split("|");
    const b = now.split("|");
    drift.push(
      `.${name} {${PROPS.map((p, i) =>
        a[i] === b[i] ? null : `${p}: ${a[i]} -> ${b[i]}`,
      )
        .filter(Boolean)
        .join("; ")}}`,
    );
  }
  expect(
    drift,
    `${drift.length} Projects class(es) changed appearance`,
  ).toEqual([]);
}
