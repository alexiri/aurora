import { expect } from "vitest";

/**
 * A WebSocket stand-in that records what the terminal tab sends. The tab assigns its handlers right after the
 * constructor, so `open` fires on a microtask. Set `expectUrl` to assert the socket's URL.
 */
export class RecordingSocket {
  static OPEN = 1;
  static instances: RecordingSocket[] = [];
  static expectUrl: string | null = null;
  readyState = RecordingSocket.OPEN;
  binaryType = "blob";
  sent: Array<Uint8Array | string> = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  constructor(url: string) {
    if (RecordingSocket.expectUrl !== null) expect(url).toBe(RecordingSocket.expectUrl);
    RecordingSocket.instances.push(this);
    queueMicrotask(() => this.onopen?.());
  }
  send(data: Uint8Array | string) {
    this.sent.push(data);
  }
  close() {}
}
