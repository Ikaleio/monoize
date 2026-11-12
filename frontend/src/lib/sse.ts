import { useEffect, useRef, useState } from "react";
import useSWRSubscription from "swr/subscription";
import type { SWRSubscriptionOptions } from "swr/subscription";
import type { RequestLog } from "./api";

const INITIAL_RECONNECT_DELAY_MS = 1_000;
const MAX_RECONNECT_DELAY_MS = 30_000;
const STALE_STREAM_TIMEOUT_MS = 45_000;

export type RequestLogStreamEvent =
  | { type: "log_batch"; logs: RequestLog[] }
  | { type: "capture_ready"; request_id: string; user_id: string }
  | { type: "resync" };

const REQUEST_LOG_STREAM_KEY = "/dashboard/request-logs/stream-subscription";

export function useRequestLogSSE(
  enabled: boolean,
  onEvent: (event: RequestLogStreamEvent) => void,
) {
  const [connected, setConnected] = useState(false);
  const onEventRef = useRef(onEvent);
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  const subscribe = (
    _key: string,
    { next }: SWRSubscriptionOptions<RequestLogStreamEvent, Error>,
  ) => {
    let cancelled = false;
    let reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let staleTimer: ReturnType<typeof setTimeout> | null = null;
    let controller: AbortController | null = null;
    let reconnectImmediately = false;

    // Deliver every frame before React can coalesce subscription state updates.
    const publish = (event: RequestLogStreamEvent) => {
      onEventRef.current(event);
      next(null, event);
    };

    const clearReconnectTimer = () => {
      if (!reconnectTimer) {
        return;
      }

      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    };

    const clearStaleTimer = () => {
      if (!staleTimer) {
        return;
      }

      clearTimeout(staleTimer);
      staleTimer = null;
    };

    const scheduleReconnect = (delayOverrideMs?: number) => {
      if (cancelled) {
        return;
      }

      setConnected(false);
      const delay = delayOverrideMs ?? reconnectDelayMs;
      if (delayOverrideMs === undefined) {
        reconnectDelayMs = Math.min(reconnectDelayMs * 2, MAX_RECONNECT_DELAY_MS);
      }
      clearReconnectTimer();
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        void connect();
      }, delay);
    };

    const armStaleTimer = () => {
      clearStaleTimer();
      if (typeof document !== "undefined" && document.visibilityState !== "visible") {
        return;
      }

      staleTimer = setTimeout(() => {
        if (cancelled) {
          return;
        }

        reconnectImmediately = true;
        publish({ type: "resync" });
        controller?.abort();
        if (!controller) {
          scheduleReconnect(0);
        }
      }, STALE_STREAM_TIMEOUT_MS);
    };

    const restartNow = () => {
      if (cancelled) {
        return;
      }

      publish({ type: "resync" });
      reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
      reconnectImmediately = true;
      setConnected(false);
      controller?.abort();
      if (!controller) {
        scheduleReconnect(0);
      }
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
          publish({ type: "log_batch", logs });
        } catch {
          return;
        }

        return;
      }

      if (eventName === "capture_ready") {
        try {
          const payload: unknown = JSON.parse(dataLines.join("\n"));
          if (
            typeof payload === "object" &&
            payload !== null &&
            "request_id" in payload &&
            typeof payload.request_id === "string" &&
            "user_id" in payload &&
            typeof payload.user_id === "string"
          ) {
            publish({
              type: "capture_ready",
              request_id: payload.request_id,
              user_id: payload.user_id,
            });
          }
        } catch {
          return;
        }
        return;
      }

      if (eventName === "resync") {
        publish({ type: "resync" });
      }
    };

    const connect = async () => {
      if (cancelled) {
        return;
      }

      controller = new AbortController();
      const decoder = new TextDecoder();

      try {
        clearStaleTimer();
        const response = await fetch("/api/dashboard/request-logs/stream", {
          credentials: "include",
          signal: controller.signal,
        });

        if (!response.ok || !response.body) {
          throw new Error(`SSE stream failed with status ${response.status}`);
        }

        setConnected(true);
        reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
        armStaleTimer();

        const reader = response.body.getReader();
        let buffer = "";

        while (!cancelled) {
          const { done, value } = await reader.read();
          if (done) {
            break;
          }

          armStaleTimer();
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
        controller = null;
        clearStaleTimer();
        if (!cancelled) {
          const delayOverrideMs = reconnectImmediately ? 0 : undefined;
          reconnectImmediately = false;
          scheduleReconnect(delayOverrideMs);
        }
      }
    };

    const handleVisibilityChange = () => {
      if (document.visibilityState !== "visible") {
        clearStaleTimer();
        return;
      }

      restartNow();
    };

    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", handleVisibilityChange);
    }

    void connect();

    return () => {
      cancelled = true;
      setConnected(false);
      clearReconnectTimer();
      clearStaleTimer();
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", handleVisibilityChange);
      }
      controller?.abort();
    };
  };

  const { error } = useSWRSubscription<RequestLogStreamEvent, Error>(
    enabled ? REQUEST_LOG_STREAM_KEY : null,
    subscribe,
  );

  return { connected, error };
}
