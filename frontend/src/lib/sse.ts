import { useState } from "react";
import useSWRSubscription from "swr/subscription";
import type { SWRSubscriptionOptions } from "swr/subscription";
import { api } from "./api";
import type { RequestLog } from "./api";

const INITIAL_RECONNECT_DELAY_MS = 1_000;
const MAX_RECONNECT_DELAY_MS = 30_000;

export type RequestLogStreamEvent =
  | { type: "log_batch"; logs: RequestLog[] }
  | { type: "resync" };

const REQUEST_LOG_STREAM_KEY = "/dashboard/request-logs/stream-subscription";

export function useRequestLogSSE(enabled: boolean) {
  const [connected, setConnected] = useState(false);

  const subscribe = (
    _key: string,
    { next }: SWRSubscriptionOptions<RequestLogStreamEvent, Error>,
  ) => {
    let cancelled = false;
    let reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let controller: AbortController | null = null;

    const clearReconnectTimer = () => {
      if (!reconnectTimer) {
        return;
      }

      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    };

    const parseEventBlock = (block: string) => {
      if (!block.trim()) {
        return;
      }

      let eventName = "";
      const dataLines: string[] = [];

      for (const line of block.split("\n")) {
        if (line.startsWith(":")) {
          continue;
        }

        if (line.startsWith("event:")) {
          eventName = line.slice("event:".length).trim();
          continue;
        }

        if (line.startsWith("data:")) {
          dataLines.push(line.slice("data:".length).trimStart());
        }
      }

      if (!eventName) {
        return;
      }

      if (eventName === "log_batch") {
        const payload = dataLines.join("\n");
        if (!payload) {
          return;
        }

        try {
          const logs = JSON.parse(payload) as RequestLog[];
          next(null, { type: "log_batch", logs });
        } catch {
          return;
        }

        return;
      }

      if (eventName === "resync") {
        next(null, { type: "resync" });
      }
    };

    const scheduleReconnect = () => {
      if (cancelled) {
        return;
      }

      setConnected(false);
      const delay = reconnectDelayMs;
      reconnectDelayMs = Math.min(reconnectDelayMs * 2, MAX_RECONNECT_DELAY_MS);
      clearReconnectTimer();
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        void connect();
      }, delay);
    };

    const connect = async () => {
      if (cancelled) {
        return;
      }

      const token = api.getToken();
      if (!token) {
        scheduleReconnect();
        return;
      }

      controller = new AbortController();
      const decoder = new TextDecoder();

      try {
        const response = await fetch("/api/dashboard/request-logs/stream", {
          headers: {
            Authorization: `Bearer ${token}`,
          },
          signal: controller.signal,
        });

        if (!response.ok || !response.body) {
          throw new Error(`SSE stream failed with status ${response.status}`);
        }

        setConnected(true);
        reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;

        const reader = response.body.getReader();
        let buffer = "";

        while (!cancelled) {
          const { done, value } = await reader.read();
          if (done) {
            break;
          }

          buffer += decoder.decode(value, { stream: true }).replaceAll("\r", "");

          let eventBoundary = buffer.indexOf("\n\n");
          while (eventBoundary !== -1) {
            const rawEvent = buffer.slice(0, eventBoundary);
            buffer = buffer.slice(eventBoundary + 2);
            parseEventBlock(rawEvent);
            eventBoundary = buffer.indexOf("\n\n");
          }
        }

        buffer += decoder.decode().replaceAll("\r", "");
        let eventBoundary = buffer.indexOf("\n\n");
        while (eventBoundary !== -1) {
          const rawEvent = buffer.slice(0, eventBoundary);
          buffer = buffer.slice(eventBoundary + 2);
          parseEventBlock(rawEvent);
          eventBoundary = buffer.indexOf("\n\n");
        }
      } catch (streamError) {
        if (cancelled) {
          return;
        }

        if (streamError instanceof DOMException && streamError.name === "AbortError") {
          return;
        }

        next(streamError instanceof Error ? streamError : new Error("SSE stream failed"));
      } finally {
        if (!cancelled) {
          scheduleReconnect();
        }
      }
    };

    void connect();

    return () => {
      cancelled = true;
      setConnected(false);
      clearReconnectTimer();
      controller?.abort();
    };
  };

  const { data: event, error } = useSWRSubscription<RequestLogStreamEvent, Error>(
    enabled ? REQUEST_LOG_STREAM_KEY : null,
    subscribe,
  );

  return { connected, event, error };
}
