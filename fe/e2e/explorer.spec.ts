import { expect, test } from "@playwright/test";
import type {
  ActivityEvent,
  AppConfig,
  List,
  Trade,
} from "@solvent/sdk/client";

const api = process.env.SOLVENT_API_URL ?? "http://localhost:8080";

test("browses real trades and activity using server cursors and filters", async ({
  page,
}) => {
  const tradesResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/v1/trades",
  );
  await page.goto("/explorer");
  const { result: trades }: { result: List<Trade> } = await (
    await tradesResponse
  ).json();
  expect(trades.items.length).toBeGreaterThan(0);
  await expect(
    page.getByRole("link", { name: `Open trade ${trades.items[0].id}` }),
  ).toBeVisible();

  if (trades.next_cursor) {
    const olderResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).searchParams.get("cursor") ===
        trades.next_cursor,
    );
    await page.getByRole("button", { name: "Next →" }).click();
    const { result: older }: { result: List<Trade> } = await (
      await olderResponse
    ).json();
    await expect(page.getByRole("link", { name: /^Open trade / })).toHaveCount(
      older.items.length,
    );
    if (older.items.length > 0)
      await expect(
        page.getByRole("link", { name: `Open trade ${older.items[0].id}` }),
      ).toBeVisible();
    await page.getByRole("button", { name: "← Prev" }).click();
    await expect(
      page.getByRole("link", { name: `Open trade ${trades.items[0].id}` }),
    ).toBeVisible();
  }

  await page.getByRole("button", { name: "All status" }).click();
  const filteredResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).searchParams.get("status") === "declined",
  );
  await page.getByRole("button", { name: "declined", exact: true }).click();
  const { result: filtered }: { result: List<Trade> } = await (
    await filteredResponse
  ).json();
  await expect(page.getByRole("link", { name: /^Open trade / })).toHaveCount(
    filtered.items.length,
  );
  await expect(page.getByRole("button", { name: "← Prev" })).toBeDisabled();

  const activityResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/v1/activity",
  );
  await page.getByRole("button", { name: "Activity", exact: true }).click();
  const { result: activity }: { result: List<ActivityEvent> } = await (
    await activityResponse
  ).json();
  const activityLinks = page.getByRole("link", {
    name: /^View .* transaction /,
  });
  await expect(activityLinks).toHaveCount(activity.items.length);
  if (activity.next_cursor) {
    const nextResponse = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        url.pathname === "/v1/activity" &&
        url.searchParams.get("cursor") === activity.next_cursor
      );
    });
    await page.getByRole("button", { name: "Load more" }).click();
    const { result: next }: { result: List<ActivityEvent> } = await (
      await nextResponse
    ).json();
    await expect(activityLinks).toHaveCount(
      activity.items.length + next.items.length,
    );
  }
  await page.screenshot({
    path: "test-results/explorer-activity.png",
    fullPage: true,
    animations: "disabled",
  });
});

test("opens a settled trade directly, reloads, and returns to Explorer", async ({
  page,
  request,
}) => {
  const response = await request.get(
    `${api}/v1/trades?status=confirmed&limit=1`,
  );
  expect(response.ok()).toBeTruthy();
  const { result: trades }: { result: List<Trade> } = await response.json();
  expect(trades.items.length).toBeGreaterThan(0);
  const { result: trade }: { result: Trade } = await (
    await request.get(`${api}/v1/trades/${trades.items[0].id}`)
  ).json();
  const { result: config }: { result: AppConfig } = await (
    await request.get(`${api}/v1/config`)
  ).json();
  const txUrl = `${config.block_explorer_url?.replace(/\/$/, "")}/tx/${trade.tx_hash}`;

  await page.goto(`/explorer/trades/${trade.id}`);
  await expect(
    page.getByRole("link", { name: "View transaction" }),
  ).toHaveAttribute("href", txUrl);
  await expect(page.getByText("In → out", { exact: true })).toBeVisible();
  await expect(
    page.getByText(`${trade.lifecycle?.length ?? 0} of 6 stages complete`),
  ).toBeVisible();
  if (trade.legs?.[0]) {
    await expect(
      page.locator(`a[href$="/strategies/${trade.legs[0].strategy_hash}"]`),
    ).toBeVisible();
  }
  await page.screenshot({
    path: "test-results/explorer-trade.png",
    fullPage: true,
    animations: "disabled",
  });
  await page.reload();
  await expect(
    page.getByRole("link", { name: "View transaction" }),
  ).toHaveAttribute("href", txUrl);
  const listResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/v1/trades",
  );
  await page.getByRole("link", { name: "Back to Explorer" }).click();
  await expect(page).toHaveURL(/\/explorer$/);
  const { result: pageTrades }: { result: List<Trade> } = await (
    await listResponse
  ).json();
  await expect(page.getByRole("link", { name: /^Open trade / })).toHaveCount(
    pageTrades.items.length,
  );
});
