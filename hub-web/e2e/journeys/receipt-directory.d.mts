export type ReceiptIdentity = { project: string; titlePath: readonly string[] };
export function showRecordingChapter(page: import('@playwright/test').Page, title: string, options?: { duration?: number }): Promise<void>;
export function receiptPartName(title: string, identity: ReceiptIdentity): string;
export function claimReceiptDirectory(directory: string, title: string, identity?: ReceiptIdentity): void;
export function captureMask(raw: string | undefined): string[] | undefined;
export function captureStageScreenshot(page: import('@playwright/test').Page, path: string, masks: string[] | undefined, maxHtmlBytes?: number): Promise<Buffer>;
