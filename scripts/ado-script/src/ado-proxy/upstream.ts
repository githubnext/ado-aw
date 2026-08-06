/**
 * Egress through Squid.
 *
 * The sidecar has no direct internet route: Squid is the only dual-homed
 * container on the AWF network, so every upstream connection — tunnelled or
 * intercepted — starts with a CONNECT to Squid. That keeps Squid's existing
 * domain policy and any configured global upstream proxy in force for traffic
 * this proxy forwards, exactly as for traffic it never sees.
 */
import { connect as netConnect, type Socket } from "node:net";

export class UpstreamError extends Error {}

interface SquidAddress {
  readonly host: string;
  readonly port: number;
}

/** Parse the configured Squid URL into a host and port. */
export function parseUpstreamProxy(raw: string): SquidAddress {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new UpstreamError(`upstream proxy ${JSON.stringify(raw)} is not a URL`);
  }
  if (url.protocol !== "http:") {
    // An HTTPS-to-Squid hop would need its own trust configuration and buys
    // nothing on an internal container network.
    throw new UpstreamError(`upstream proxy must be http://, got ${url.protocol}`);
  }
  return { host: url.hostname, port: url.port === "" ? 3128 : Number(url.port) };
}

const CONNECT_TIMEOUT_MS = 30_000;

/**
 * Open a tunnel to `host:port` through Squid.
 *
 * Resolves with the raw socket once Squid answers `200`. A non-200 is surfaced
 * as an {@link UpstreamError} carrying only the status line, because Squid's
 * error bodies can echo request details.
 */
export function connectThroughProxy(
  proxy: SquidAddress,
  host: string,
  port: number,
): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = netConnect({ host: proxy.host, port: proxy.port });
    let settled = false;
    let buffered = "";

    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      socket.destroy();
      reject(error);
    };

    const timer = setTimeout(() => {
      fail(new UpstreamError(`timed out opening a tunnel to ${host}:${port}`));
    }, CONNECT_TIMEOUT_MS);
    timer.unref?.();

    const onData = (chunk: Buffer): void => {
      buffered += chunk.toString("latin1");
      const headerEnd = buffered.indexOf("\r\n\r\n");
      if (headerEnd === -1) {
        if (buffered.length > 16 * 1024) {
          fail(new UpstreamError("upstream proxy sent an oversized CONNECT response"));
        }
        return;
      }

      const statusLine = buffered.slice(0, buffered.indexOf("\r\n"));
      const status = Number(statusLine.split(" ")[1]);
      if (status !== 200) {
        fail(new UpstreamError(`upstream proxy refused CONNECT: ${statusLine.trim()}`));
        return;
      }

      settled = true;
      clearTimeout(timer);
      socket.removeListener("data", onData);
      socket.removeListener("error", fail);

      // Squid should not send payload bytes before the tunnel opens; if it did,
      // they belong to the tunnelled stream and must not be dropped.
      const leftover = buffered.slice(headerEnd + 4);
      if (leftover.length > 0) socket.unshift(Buffer.from(leftover, "latin1"));
      resolve(socket);
    };

    socket.on("error", fail);
    socket.on("data", onData);
    socket.on("connect", () => {
      socket.write(
        `CONNECT ${host}:${port} HTTP/1.1\r\nHost: ${host}:${port}\r\n` +
          "Proxy-Connection: keep-alive\r\n\r\n",
      );
    });
  });
}
