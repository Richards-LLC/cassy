const { expect, test } = require('@playwright/test');

test('deterministic', () => expect(2 + 2).toBe(4));
test('seeded one in three', ({}, testInfo) => {
  expect(testInfo.repeatEachIndex % 3).not.toBe(2);
});
