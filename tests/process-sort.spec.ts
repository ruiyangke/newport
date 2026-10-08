import { test, expect } from "@playwright/test";
import { installAppFixture } from "./fixtures/appFixture";

/**
 * Sorting the process list must not move the page. On WebKit the row
 * replacement adjusts the workspace scroller with no JS scroll call, focus
 * change, or geometry change involved; the sort commit captures the offset
 * and repairs it right after (see useScrollGuard).
 */
test("sorting processes keeps the page scroll position", async ({ page }) => {
  await installAppFixture(page);
  await page.goto("/?fixture=large-lists");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("table", { name: "Processes" })).toBeVisible();
  const scroller = page.locator('[data-slot="workspace-scroll"]');
  const height = () =>
    scroller.evaluate((el: HTMLElement) => el.scrollHeight);
  // Let lazy chunks land: only start once the page height settles, so a
  // late pop-in cannot be mistaken for the sort.
  await page.waitForFunction(
    () => {
      const ws = document.querySelector('[data-slot="workspace-scroll"]')!;
      const seen = ((window as any).__heights ??= []) as number[];
      seen.push(ws.scrollHeight);
      if (seen.length > 6) seen.shift();
      return seen.length === 6 && seen.every((h) => h === ws.scrollHeight);
    },
    null,
    { polling: 250, timeout: 15000 },
  );
  await scroller.evaluate((el) => el.scrollTo(0, el.scrollHeight));
  const scrolled = await scroller.evaluate((el: HTMLElement) => el.scrollTop);
  expect(scrolled).toBeGreaterThan(0);
  expect(await height()).toBeGreaterThan(0);
  const memory = page.getByRole("columnheader", { name: /Memory/ });
  await memory.locator("button").click();
  await expect(memory).toHaveAttribute("aria-sort", "descending");
  // Read back immediately after the commit: the jump under test lands
  // synchronously with it.
  expect(await scroller.evaluate((el: HTMLElement) => el.scrollTop)).toBe(
    scrolled,
  );
  await memory.locator("button").click();
  await expect(memory).toHaveAttribute("aria-sort", "ascending");
  expect(await scroller.evaluate((el: HTMLElement) => el.scrollTop)).toBe(
    scrolled,
  );
});
