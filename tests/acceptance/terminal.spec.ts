import { expect, test } from "@playwright/test";

test("user can connect to a terminal session and run a command", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("link", { name: "temporary terminal POC" }).click();

  await expect(
    page.getByRole("heading", { name: "Interactive shell over the session." }),
  ).toBeVisible();

  await page.getByRole("button", { exact: true, name: "Connect" }).click();

  const status = page.locator(".terminal-poc__status");
  const terminal = page.locator(".terminal-surface__viewport");

  await expect(status).toHaveText("Connected.", { timeout: 20_000 });
  await expect(terminal).toContainText("Interactive shell connected.", {
    timeout: 20_000,
  });

  await terminal.click();
  await page.keyboard.type("printf playwright-terminal-ok\\r");

  await expect(terminal).toContainText("playwright-terminal-ok");
});
