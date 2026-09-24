// Types for checkout-ports.mjs (kept as plain JavaScript; see there).
export declare const PORT_FLOOR: number;
export declare const PORT_CEILING: number;
/** The fixture and journey ports for the checkout at `dir` (the hub-web directory). */
export declare function checkoutPorts(dir: string): { fixtures: number; journeys: number };
