import { useLayoutEffect, useRef, useState } from "react";
import useSWRSubscription from "swr/subscription";
import type { SWRSubscriptionOptions } from "swr/subscription";
import type { RequestLog } from "./api";

const INITIAL_RECONNECT_DELAY_MS = 1_000;
const MAX_RECONNECT_DELAY_MS = 30_000;
const CONNECT_TIMEOUT_MS = 30_000;
const STALE_STREAM_TIMEOUT_MS = 45_000;

export type RequestLogStreamEvent =
  | { type: "log_batch"; logs: RequestLog[]; initial?: boolean }
  | { type: "pending_empty" }
  | { type: "capture_ready"; request_id: string; user_id: string }
  | { type: "resync" };

const REQUEST_LOG_STREAM_KEY = "/dashboard/request-logs/stream-subscription";

export function useRequestLogSSE(
  enabled: boolean,
  onEvent: (event: RequestLogStreamEvent) => void,
) {
  const [connected, setConnected] = useState(false);
  const onEventRef = useRef(onEvent);
  useLayoutEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  const subscribe = (
    _key: string,
    { next }: SWRSubscriptionOptions<RequestLogStreamEvent, Error>,
  ) => {
    let cancelled = false;
    let reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let connectTimer: ReturnType<typeof setTimeout> | null = null;
    let staleTimer: ReturnType<typeof setTimeout> | null = null;
    let controller: AbortController | null = null;

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

    const clearConnectTimer = () => {
      if (!connectTimer) {
        return;
      }

      clearTimeout(connectTimer);
      connectTimer = null;
    };

    const clearStaleTimer = () => {
      if (!staleTimer) {
        return;
      }

      clearTimeout(staleTimer);
      staleTimer = null;
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

    const armConnectTimer = (target: AbortController) => {
      clearConnectTimer();
      connectTimer = setTimeout(() => {
        connectTimer = null;
        if (cancelled || controller !== target) {
          return;
        }
        target.abort();
      }, CONNECT_TIMEOUT_MS);
    };

    const armStaleTimer = (target: AbortController) => {
      clearStaleTimer();
      if (typeof document !== "undefined" && document.visibilityState !== "visible") {
        return;
      }

      staleTimer = setTimeout(() => {
        staleTimer = null;
        if (cancelled || controller !== target) {
          return;
        }
        restartNow();
      }, STALE_STREAM_TIMEOUT_MS);
    };

    const restartNow = () => {
      if (cancelled) {
        return;
      }

      clearReconnectTimer();
      clearConnectTimer();
      clearStaleTimer();
      reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
      setConnected(false);
      const previous = controller;
      controller = null;
      previous?.abort();
      publish({ type: "resync" });
      void connect();
    };

    const parseEventBlock = (block: string, isInitialFrame: boolean): boolean => {
      if (!block.trim()) {
        return false;
      }

      let eventName = "";
      let sawReadyComment = false;
      const dataLines: string[] = [];

      for (const line of block.split("\n")) {
        if (line.startsWith(":")) {
          if (line.slice(1).trim() === "ready") {
            sawReadyComment = true;
          }
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
        if (isInitialFrame && sawReadyComment) {
          publish({ type: "pending_empty" });
        }
        return true;
      }

      if (eventName === "log_batch") {
        const payload = dataLines.join("\n");
        if (!payload) {
          return true;
        }

        try {
          const logs = JSON.parse(payload) as RequestLog[];
          publish(
            isInitialFrame
              ? { type: "log_batch", logs, initial: true }
              : { type: "log_batch", logs },
          );
        } catch {
          return true;
        }

        return true;
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
          return true;
        }
        return true;
      }

      if (eventName === "resync") {
        publish({ type: "resync" });
      }
      return true;
    };

    const connect = async () => {
      if (cancelled) {
        return;
      }

      clearReconnectTimer();
      const previous = controller;
      const currentController = new AbortController();
      controller = currentController;
      previous?.abort();

      const decoder = new TextDecoder();
      let reader: ReadableStreamDefaultReader<Uint8Array> | null = null;

      try {
        clearStaleTimer();
        armConnectTimer(currentController);
        const response = await fetch("/api/dashboard/request-logs/stream", {
          credentials: "include",
          cache: "no-store",
          headers: {
            Accept: "text/event-stream",
            "Cache-Control": "no-transform",
          },
          signal: currentController.signal,
        });
        clearConnectTimer();

        if (cancelled || controller !== currentController) {
          return;
        }

        if (!response.ok || !response.body) {
          throw new Error(`SSE stream failed with status ${response.status}`);
        }

        setConnected(true);
        armStaleTimer(currentController);

        reader = response.body.getReader();
        let buffer = "";
        let sawInitialFrame = false;

        while (!cancelled && controller === currentController) {
          const { done, value } = await reader.read();
          if (done) {
            break;
          }

          if (cancelled || controller !== currentController) {
            break;
          }

          reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
          armStaleTimer(currentController);
          buffer += decoder.decode(value, { stream: true }).replaceAll("\r", "");

          let eventBoundary = buffer.indexOf("\n\n");
          while (eventBoundary !== -1) {
            const rawEvent = buffer.slice(0, eventBoundary);
            buffer = buffer.slice(eventBoundary + 2);
            if (parseEventBlock(rawEvent, !sawInitialFrame)) {
              sawInitialFrame = true;
            }
            eventBoundary = buffer.indexOf("\n\n");
          }
        }

        if (!cancelled && controller === currentController) {
          buffer += decoder.decode().replaceAll("\r", "");
          let eventBoundary = buffer.indexOf("\n\n");
          while (eventBoundary !== -1) {
            const rawEvent = buffer.slice(0, eventBoundary);
            buffer = buffer.slice(eventBoundary + 2);
            if (parseEventBlock(rawEvent, !sawInitialFrame)) {
              sawInitialFrame = true;
            }
            eventBoundary = buffer.indexOf("\n\n");
          }
        }
      } catch (streamError) {
        if (cancelled || controller !== currentController) {
          return;
        }

        if (streamError instanceof DOMException && streamError.name === "AbortError") {
          return;
        }

        next(streamError instanceof Error ? streamError : new Error("SSE stream failed"));
      } finally {
        if (reader) {
          void reader.cancel().catch(() => {});
        }
        if (controller === currentController) {
          controller = null;
          clearConnectTimer();
          clearStaleTimer();
          if (!cancelled) {
            scheduleReconnect();
          }
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

    const handleOnline = () => {
      restartNow();
    };

    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", handleVisibilityChange);
    }
    if (typeof window !== "undefined") {
      window.addEventListener("online", handleOnline);
    }

    void connect();

    return () => {
      cancelled = true;
      setConnected(false);
      clearReconnectTimer();
      clearConnectTimer();
      clearStaleTimer();
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", handleVisibilityChange);
      }
      if (typeof window !== "undefined") {
        window.removeEventListener("online", handleOnline);
      }
      const active = controller;
      controller = null;
      active?.abort();
    };
  };

  const { error } = useSWRSubscription<RequestLogStreamEvent, Error>(
    enabled ? REQUEST_LOG_STREAM_KEY : null,
    subscribe,
  );

  return { connected, error };
}
