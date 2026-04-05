import { expect, test } from "@playwright/test";
import { expectApiReady } from "./support";

test("user can open the app shell", async ({ page }) => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", {
      name: "The frontend stays deliberately small and same-origin.",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "temporary run POC" }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "temporary terminal POC" }),
  ).toBeVisible();
});

test("service reports its readiness and version endpoints", async ({
  request,
}) => {
  await expectApiReady(request);
});
