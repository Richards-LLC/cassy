// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

import { test, expect } from '@playwright/test';

test.describe('Failed connection', () => {
  test('Failed connection explains the third attempt and offers recovery controls', async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=connection-failed-retry.
    await page.goto('/?fixture=connection-failed-retry');
    await expect(page.getByText('Connection failed — retry available.')).toBeVisible();
    const attempts = page.getByRole('list', { name: 'Connection attempts' });
    await expect(attempts).toBeVisible();
    await expect(attempts.getByRole('listitem')).toHaveCount(3);
    await expect(attempts.getByRole('listitem').nth(0)).toContainText('Earlier attempts');
    await expect(attempts.getByRole('listitem').nth(0)).toContainText('2 attempts did not reach a live session');
    await expect(attempts.getByRole('listitem').nth(1)).toContainText('Attempt 3');
    await expect(attempts.getByRole('listitem').nth(1)).toContainText('Connection failed');
    await expect(attempts.getByRole('listitem').nth(2)).toContainText('The machine did not answer its hub address.');

    // 2. Locate and click Retry, then locate and click Diagnose.
    const retry = page.getByRole('button', { name: 'Retry' });
    const diagnose = page.getByRole('button', { name: 'Diagnose' });
    await expect(retry).toBeVisible();
    await expect(retry).toBeEnabled();
    await expect(diagnose).toBeVisible();
    await expect(diagnose).toBeEnabled();
    await retry.click();
    await diagnose.click();
    await expect(page).toHaveURL(/\?fixture=connection-failed-retry$/);
    await expect(page.getByText('Connection failed — retry available.')).toBeVisible();
    await expect(attempts.getByRole('listitem').nth(2)).toContainText('The machine did not answer its hub address.');
  });
});
