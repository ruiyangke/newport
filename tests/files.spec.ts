import { test, expect } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";

// Two real PDF pages; a valid cross-reference table keeps the fixture independent
// of PDF.js recovery behavior.
function pdfFixture() {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /Font << /F1 7 0 R >> >> /Contents 4 0 R >>",
    "<< /Length 54 >>\nstream\nBT /F1 20 Tf 40 220 Td (Porthop PDF preview) Tj ET\nendstream",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Rotate 90 /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>",
    "<< /Length 44 >>\nstream\nBT /F1 20 Tf 40 220 Td (Second page) Tj ET\nendstream",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  for (let index = 0; index < objects.length; index++) {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${objects[index]}\nendobj\n`;
  }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  offsets.slice(1).forEach((offset) => {
    pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  });
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(pdf).toString("base64");
}
test.beforeEach(async ({ page, baseURL }) => {
  const csp = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).app
    .security.csp;
  await page.route("**/*", async (route) => {
    if (
      !baseURL?.endsWith(":1431") ||
      route.request().resourceType() !== "document"
    )
      return route.continue();
    const response = await route.fetch();
    await route.fulfill({
      response,
      headers: { ...response.headers(), "content-security-policy": csp },
    });
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(
    ({ pdf }) => {
      const state = {
        config: {
          servers: [
            {
              id: "files-server",
              name: "Development",
              sshUser: "developer",
              sshHost: "dev.example.com",
              sshPort: 22,
              identityFile: null,
              authMethod: "publicKey",
            },
          ],
          tunnels: [],
        },
        runtime: { tunnels: {}, clipboard: {}, health: {} },
        loadError: null,
      };
      const entry = (
        name: string,
        kind = "file",
        folder = "/home/developer",
      ) => ({
        name,
        path: `${folder}/${name}`,
        kind,
        size: kind === "directory" ? null : 2048,
        modified: 1789983160,
        permissions: "644",
      });
      let uploaded = false;
      const cancelled: string[] = [];
      const transfers: string[] = [];
      const pending = new Map<string, (value: string | null) => void>();
      Object.assign(window, {
        isTauri: true,
        __fileCancelled: cancelled,
        __fileTransfers: transfers,
        __TAURI_INTERNALS__: {
          metadata: { currentWindow: { label: "main" } },
          invoke: async (cmd: string, args: Record<string, any>) => {
            if (cmd === "snapshot") return structuredClone(state);
            if (cmd === "get_sidebar_width") return 200;
            if (cmd === "files_cancel") {
              cancelled.push(args.operation);
              pending.get(args.operation)?.(null);
              return;
            }
            if (cmd === "files_list") {
              const path = args.path === "." ? "/home/developer" : args.path;
              if (
                path === "/denied" ||
                (path.endsWith("/Documents") && (window as any).__denyDocuments)
              )
                throw new Error("Permission denied");
              if (path === "/slow") {
                await new Promise((resolve) => setTimeout(resolve, 400));
                return { path, entries: [] };
              }
              return {
                path,
                entries:
                  path === "/home/developer"
                    ? [
                        entry("Documents", "directory"),
                        entry("logs", "directory"),
                        entry(".env"),
                        entry("README.md"),
                        entry("report.pdf"),
                        entry("photo.png"),
                        entry("archive.bin"),
                        entry("main.rs"),
                        entry("app.tsx"),
                        entry("Dockerfile"),
                        entry("Cargo.toml"),
                        entry("backup.tar.gz"),
                        ...(uploaded ? [entry("uploaded.txt")] : []),
                      ]
                    : path === "/home/developer/logs"
                      ? Array.from({ length: 235 }, (_, i) =>
                          entry(
                            `application-${String(i).padStart(3, "0")}.log`,
                            "file",
                            path,
                          ),
                        )
                      : path === "/home/developer/Documents"
                        ? [entry("design-notes.md", "file", path)]
                        : [entry("home", "directory", path)],
              };
            }
            if (cmd === "files_preview") {
              if (args.path.endsWith(".pdf"))
                return { kind: "pdf", mime: "application/pdf", content: pdf };
              if (args.path.endsWith(".bin"))
                throw new Error(
                  "This binary file cannot be previewed. Download it to open it.",
                );
              if (args.path.endsWith(".png"))
                return {
                  kind: "image",
                  mime: "image/png",
                  content:
                    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j1ioAAAAASUVORK5CYII=",
                };
              return {
                kind: "text",
                mime: "text/plain",
                content:
                  "# Deployment notes\n\n<script>window.pwned = true</script>\n\nRun the service after reviewing the configuration.\n",
              };
            }
            if (cmd === "files_upload") {
              uploaded = true;
              transfers.push("upload");
              return "/home/developer/uploaded.txt";
            }
            if (cmd === "files_download") {
              transfers.push("download");
              return new Promise((resolve) =>
                pending.set(args.operation, resolve),
              );
            }
            if (cmd === "files_progress")
              return { name: "report.pdf", completed: 1024, total: 4096 };
            if (cmd === "cockpit_history") return [];
            return null;
          },
        },
      });
    },
    { pdf: pdfFixture() },
  );
  await page.goto("/");
  await page.getByRole("tab", { name: "Files", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Remote files" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "README.md", exact: true }),
  ).toBeVisible();
});

test("files browse folders, paginate long lists, filter and recover from errors", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Expand Home", exact: true }).click();
  await page
    .getByRole("navigation", { name: "Folder tree" })
    .getByRole("button", { name: "Documents", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "design-notes.md", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Home folder", exact: true }).click();
  await page
    .locator(".file-list")
    .getByRole("button", { name: "logs", exact: true })
    .click();
  await expect(page.getByText("235 items", { exact: true })).toBeVisible();
  await expect(page.locator(".file-list tbody tr")).toHaveCount(100);
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "application-100.log", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Refresh folder", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "application-100.log", exact: true }),
  ).toBeVisible();
  const scrollRegion = page.getByRole("region", {
    name: "Files in current folder",
    exact: true,
  });
  await scrollRegion.focus();
  await scrollRegion.press("End");
  await expect
    .poll(() => scrollRegion.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  if (page.context().browser()?.browserType().name() === "webkit") {
    await page.screenshot({
      path: "test-results/screenshots/files-refresh-focus.png",
    });
  }
  await page
    .getByRole("textbox", { name: "Filter files", exact: true })
    .fill("234");
  await expect(page.locator(".file-list tbody tr")).toHaveCount(1);
  await page
    .getByRole("textbox", { name: "Remote path", exact: true })
    .fill("/denied");
  await page
    .getByRole("textbox", { name: "Remote path", exact: true })
    .press("Enter");
  await expect(page.getByRole("alert")).toContainText("Permission denied");
  await page.getByRole("button", { name: "Home folder", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).__denyDocuments = true;
  });
  await page
    .locator(".file-list")
    .getByRole("button", { name: "Documents", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText(
    "/home/developer/Documents",
  );
  if (page.context().browser()?.browserType().name() === "webkit") {
    await page.screenshot({ path: "test-results/screenshots/files-retry.png" });
  }
  await page.evaluate(() => {
    (window as any).__denyDocuments = false;
  });
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "design-notes.md", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Home folder", exact: true }).click();

  await expect(
    page.getByRole("button", { name: ".env", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Hidden files", exact: true }).click();
  await expect(
    page.getByRole("button", { name: ".env", exact: true }),
  ).toBeVisible();
});

test("files preview inert text, real PDF pages and images; upload and cancel download", async ({
  page,
}) => {
  await page.getByRole("button", { name: "README.md", exact: true }).click();
  await expect(page.getByRole("dialog").locator("pre")).toContainText(
    "<script>window.pwned = true</script>",
  );
  expect(await page.evaluate(() => (window as any).pwned)).toBeUndefined();
  await page.getByRole("button", { name: "Close dialog", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "report.pdf", exact: true }).click();
  await expect(page.getByText("Page 1 of 2", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Next PDF page" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Next PDF page" }).click();
  await expect(page.getByText("Page 2 of 2", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Previous PDF page" }),
  ).toBeEnabled();
  const ink = await page
    .locator(".pdf-sheet > canvas")
    .evaluate((canvas: HTMLCanvasElement) => {
      const pixels = canvas
        .getContext("2d")!
        .getImageData(0, 0, canvas.width, canvas.height).data;
      return pixels.some((value, index) => index % 4 !== 3 && value < 100);
    });
  expect(ink).toBe(true);
  await expect(page.locator(".textLayer")).toContainText("Second page");
  await page.setViewportSize({ width: 640, height: 480 });
  await expect
    .poll(async () => {
      const canvas = await page.locator(".pdf-sheet > canvas").boundingBox();
      const text = await page.locator(".textLayer").boundingBox();
      return (
        Math.abs(canvas!.width - text!.width) < 2 &&
        Math.abs(canvas!.height - text!.height) < 2
      );
    })
    .toBe(true);
  await page.setViewportSize({ width: 960, height: 680 });

  await page.getByRole("button", { name: "Close dialog", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "photo.png", exact: true }).click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("img", { name: "photo.png", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Close dialog", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "archive.bin", exact: true }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "binary file cannot be previewed",
  );
  await page.getByRole("button", { name: "Close dialog", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "Upload", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "uploaded.txt", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Download report.pdf", exact: true })
    .click();
  await expect(
    page.getByRole("progressbar", { name: "File transfer" }),
  ).toHaveAttribute("value", "1024");
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("progressbar")).toHaveCount(0);
  expect(
    await page.evaluate(() => (window as any).__fileCancelled.length),
  ).toBeGreaterThan(0);
});

test("files layout desktop, compact dark and PDF screenshots", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "webkit");
  mkdirSync("test-results/screenshots", { recursive: true });
  await page.getByRole("button", { name: "Expand Home", exact: true }).click();
  await expect(
    page
      .getByRole("navigation", { name: "Folder tree" })
      .getByRole("button", { name: "logs", exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: "test-results/screenshots/files-desktop.png" });
  await page.getByRole("button", { name: "report.pdf", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Next PDF page" }),
  ).toBeEnabled();
  await expect(page.locator(".textLayer")).toContainText("Porthop PDF preview");
  await page.locator(".textLayer").evaluate((layer) => {
    const range = document.createRange();
    range.selectNodeContents(layer);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  expect(
    await page.evaluate(() => window.getSelection()?.toString()),
  ).toContain("Porthop PDF preview");
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  await page.keyboard.press("Meta+1");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.locator('[role="tab"][data-state="active"]')).toContainText(
    "Files",
  );
  await page.screenshot({ path: "test-results/screenshots/files-pdf.png" });
  await page.getByRole("button", { name: "Close dialog", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.setViewportSize({ width: 640, height: 480 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.screenshot({ path: "test-results/screenshots/files-compact.png" });
  await page.getByRole("button", { name: "README.md", exact: true }).click();
  await expect(page.getByRole("dialog").locator("pre")).toBeVisible();
  await page.getByRole("button", { name: "Wrap lines", exact: true }).click();
  await expect(page.getByRole("dialog").locator("pre")).toHaveCSS(
    "white-space",
    "pre-wrap",
  );
  await page.screenshot({
    path: "test-results/screenshots/files-preview-compact.png",
  });
  const dialog = await page.getByRole("dialog").boundingBox();
  expect(dialog!.y + dialog!.height).toBeLessThanOrEqual(480);
  expect(dialog!.x + dialog!.width).toBeLessThanOrEqual(640);
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  expect(overflow).toBe(false);
});
