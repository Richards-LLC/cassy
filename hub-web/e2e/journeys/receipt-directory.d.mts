export type ReceiptIdentity = { project: string; titlePath: readonly string[] };
export function receiptPartName(title: string, identity: ReceiptIdentity): string;
export function claimReceiptDirectory(directory: string, title: string, identity?: ReceiptIdentity): void;
