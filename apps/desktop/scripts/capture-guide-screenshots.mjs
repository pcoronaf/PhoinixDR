// Captures the screenshots of docs/user-guide/desktop.md from the browser
// demo (the front-end without Tauri, with demo data). Usage:
//
//   cd apps/desktop && npm run build
//   (cd dist && python3 -m http.server 8766) &
//   npx --yes playwright@1.55 install chromium      # once, or set executablePath below
//   node scripts/capture-guide-screenshots.mjs ../../docs/user-guide/images
//
// The images are then shrunk to 256 colours (see the PIL snippet in the
// commit that introduced them). The demo badge is hidden and the demo
// version replaced by the release version so the pictures match the app.
import { chromium } from "playwright";
const out = process.argv[2];
const browser = await chromium.launch({ args: ["--no-sandbox"] });
const page = await browser.newPage({ viewport: { width: 1280, height: 780 }, deviceScaleFactor: 1 });
page.on("dialog", (d) => d.accept(d.defaultValue() || "C:\\images\\stick.img"));
await page.goto("http://127.0.0.1:8766/");
await page.addStyleTag({ content: ".tag{display:none!important}" });
const tidy = () => page.evaluate(() => { for (const el of document.querySelectorAll(".byline")) el.textContent = el.textContent.replace("v0.1.0-demo", "v0.1.2"); });
const shot = async (name) => { await tidy(); await page.screenshot({ path: `${out}/${name}.png` }); console.log("captured", name); };

await page.waitForSelector(".home .choices");
await shot("01-home");
await page.getByRole("button", { name: /Physical disk/ }).click();
await page.waitForSelector(".elevate");
await shot("02-devices");
await page.getByRole("button", { name: "Back" }).click();
await page.getByRole("button", { name: /Disk image/ }).click();
await page.waitForSelector("button.primary");
await shot("03-setup");
await page.getByRole("button", { name: /^Scan/ }).click();
try { await page.waitForSelector(".progress .bar", { timeout: 3000 }); await shot("04-scanning"); } catch { console.log("scan too fast for a progress capture"); }
await page.waitForSelector(".candidates tbody tr", { timeout: 20000 });
await page.locator(".candidates tbody tr").nth(2).click();
await page.waitForSelector(".detail-body .reasons li");
await shot("05-results");
await page.getByRole("button", { name: "Preview" }).click();
await page.waitForSelector(".preview img, .preview pre", { timeout: 10000 });
await shot("06-preview");
await page.locator(".candidates tbody tr").nth(0).locator("input[type=checkbox]").check();
await page.locator(".candidates tbody tr").nth(1).locator("input[type=checkbox]").check();
await page.getByRole("button", { name: /^Recover/ }).click();
await page.waitForSelector(".modal");
await page.locator(".modal .dest input").first().fill("D:\\recovered");
await shot("07-recover");
await page.locator(".modal button.primary", { hasText: "Recover" }).click();
await page.getByRole("button", { name: "Done" }).waitFor({ timeout: 20000 });
await shot("08-recovered");
await browser.close();
