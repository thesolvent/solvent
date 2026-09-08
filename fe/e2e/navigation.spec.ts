import { expect, test } from "@playwright/test";

test("transitions from pool settlements to trade detail and through browser history", async ({
  page,
}) => {
  await page.goto("/pools/dai-usdc");
  const trade = page.getByRole("link", { name: /^Open trade / }).first();
  await expect(trade).toBeVisible();
  const destination = await trade.getAttribute("href");
  expect(destination).toBeTruthy();

  await page.clock.install();
  await page.clock.pauseAt(new Date());
  await trade.click();
  const overlay = page.locator('[class*="_wipe_"]');
  await expect(overlay).toBeAttached();
  await expect(page).toHaveURL(destination!);
  await expect(trade).toBeAttached();
  await page.clock.runFor(210);
  await expect(
    page.getByRole("link", { name: "Back to Explorer" }),
  ).toBeVisible();
  await page.clock.runFor(310);
  await expect(overlay).toHaveCount(0);

  await page.goBack();
  await expect(overlay).toBeAttached();
  await page.clock.runFor(520);
  await expect(page).toHaveURL(/\/pools\/dai-usdc$/);
  await expect(trade).toBeAttached();
  await page.goForward();
  await expect(overlay).toBeAttached();
  await page.clock.runFor(520);
  await expect(page).toHaveURL(destination!);
  await expect(overlay).toHaveCount(0);
});
