import type { IncomingMessage, ServerResponse } from "node:http";

export const LOGIN: string;
export const LIFETIME: number;
export const SIGNED_OUT: string;
export function lanAddresses(): string[];
export function readPassword(home: string | undefined): string;
export function lanGate(config: {
  home: string;
  hosts: string[];
  now?: () => number;
}): (req: IncomingMessage, res: ServerResponse) => Promise<boolean>;
