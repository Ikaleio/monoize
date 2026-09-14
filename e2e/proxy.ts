import WebSocket from "ws";
import { Recorder, SSEParser, BUDGET, ENDPOINTS, type Protocol } from "./core";

export interface ProxyConfig {
  monoize: string;
  baseUrl: string;
  upstreamType: Protocol;
  maxRequests: number;
}
export function createProxy(
  config: ProxyConfig,
  recorder: Recorder,
  control?: (req: Request) => Promise<Response>,
  port = 0,
) {
  let generations = 0;
  let fatal: string | undefined;
  const fail = (message: string) => {
    fatal ??= message;
    recorder.add("failure", { category: message });
  };
  const sockets = new Set<WebSocket>();
  const headersFor = (source: Headers) => {
    const headers = new Headers(source);
    for (const h of [
      "host",
      "connection",
      "keep-alive",
      "proxy-authenticate",
      "proxy-authorization",
      "te",
      "trailer",
      "transfer-encoding",
      "upgrade",
      "content-length",
      "accept-encoding",
    ])
      headers.delete(h);
    return headers;
  };
  const admit = (body: any, websocket = false) => {
    if (
      websocket &&
      body?.type === "response.create" &&
      body.generate === false
    )
      return true;
    if (++generations <= config.maxRequests) return true;
    fail("request_limit");
    return false;
  };
  const server = Bun.serve<{
    id: string;
    remote: WebSocket;
    queue: string[];
    ready: boolean;
  }>({
    hostname: "0.0.0.0",
    port,
    maxRequestBodySize: BUDGET,
    idleTimeout: 0,
    async fetch(req, server) {
      const url = new URL(req.url);
      if (url.pathname.startsWith("/control/"))
        return control
          ? control(req)
          : new Response("not found", { status: 404 });
      if (url.pathname === "/health")
        return Response.json({
          ok: true,
          fatal,
          generations,
          exceeded: recorder.exceeded,
        });
      if (fatal || recorder.exceeded)
        return Response.json(
          { error: { message: fatal ?? "evidence_limit" } },
          { status: 503 },
        );
      const side = url.pathname.startsWith("/upstream/")
        ? "upstream"
        : "downstream";
      const pathname =
        side === "upstream"
          ? url.pathname.slice("/upstream".length)
          : url.pathname;
      const id = crypto.randomUUID();
      if (req.headers.get("upgrade")?.toLowerCase() === "websocket") {
        if (side !== "downstream" || pathname !== "/v1/responses")
          return new Response("invalid upgrade", { status: 400 });
        const remoteURL = new URL(pathname + url.search, config.monoize);
        remoteURL.protocol = remoteURL.protocol === "https:" ? "wss:" : "ws:";
        recorder.add("upgrade", {
          side,
          id,
          path: pathname,
          headers: Object.fromEntries(req.headers),
        });
        const headers = headersFor(req.headers);
        for (const h of [
          "sec-websocket-key",
          "sec-websocket-version",
          "sec-websocket-extensions",
        ])
          headers.delete(h);
        const remote = new WebSocket(remoteURL, {
          headers: Object.fromEntries(headers),
        });
        sockets.add(remote);
        const data = { id, remote, queue: [] as string[], ready: false };
        if (server.upgrade(req, { data })) return;
        remote.close();
        return new Response("upgrade failed", { status: 502 });
      }
      const target =
        side === "upstream"
          ? config.baseUrl
          : new URL(pathname + url.search, config.monoize).toString();
      const isGeneration =
        req.method === "POST" && Object.values(ENDPOINTS).includes(pathname);
      try {
        const bytes = new Uint8Array(await req.arrayBuffer());
        let body: any;
        if (bytes.length) {
          let decoded = bytes;
          const encoding = req.headers.get("content-encoding");
          if (encoding === "zstd")
            decoded = new Uint8Array(Bun.zstdDecompressSync(bytes));
          else if (encoding === "gzip") decoded = Bun.gunzipSync(bytes);
          else if (encoding && encoding !== "identity")
            throw new Error("unsupported_request_encoding");
          if (decoded.length > BUDGET) throw new Error("evidence_limit");
          body = JSON.parse(new TextDecoder().decode(decoded));
        }
        recorder.add("request", {
          side,
          id,
          method: req.method,
          path: pathname,
          target,
          headers: Object.fromEntries(req.headers),
          body,
        });
        if (side === "downstream" && isGeneration && !admit(body))
          return new Response("request_limit", { status: 429 });
        if (
          side === "upstream" &&
          (!isGeneration || pathname !== ENDPOINTS[config.upstreamType])
        )
          throw new Error("unexpected_upstream_endpoint");
        const response = await fetch(target, {
          method: req.method,
          headers: headersFor(req.headers),
          body: bytes.length ? bytes : undefined,
          redirect: "manual",
          signal: req.signal,
        });
        recorder.add("response", {
          side,
          id,
          status: response.status,
          headers: Object.fromEntries(response.headers),
        });
        if (!response.ok) {
          recorder.add("http_error", { side, id, status: response.status });
          if (isGeneration) fail("upstream_error");
        }
        const isSSE = response.headers
          .get("content-type")
          ?.includes("text/event-stream");
        const parser = isSSE
          ? new SSEParser((event) => {
              recorder.add("event", { side, id, event });
              if (
                side === "upstream" &&
                (event.error ||
                  ["error", "response.failed", "response.incomplete"].includes(
                    event.type,
                  ))
              )
                fail("upstream_error");
            })
          : undefined;
        let text = "";
        const decoder = new TextDecoder();
        let size = 0;
        const transformed = response.body?.pipeThrough(
          new TransformStream<Uint8Array, Uint8Array>({
            transform(chunk, controller) {
              try {
                if (parser) parser.feed(chunk);
                else {
                  size += chunk.length;
                  if (size > BUDGET) throw new Error("evidence_limit");
                  text += decoder.decode(chunk, { stream: true });
                }
                if (recorder.exceeded) throw new Error("evidence_limit");
                controller.enqueue(chunk);
              } catch (e) {
                fail(String(e));
                controller.error(e);
              }
            },
            flush() {
              try {
                parser?.end();
                if (!parser) text += decoder.decode();
                if (!parser && text) {
                  let body: unknown = text;
                  try {
                    body = JSON.parse(text);
                  } catch {}
                  recorder.add("body", { side, id, body });
                }
                recorder.add("end", { side, id });
              } catch (e) {
                fail(String(e));
                throw e;
              }
            },
          }),
        );
        const reader = transformed?.getReader();
        const observed = reader
          ? new ReadableStream<Uint8Array>({
              async pull(controller) {
                try {
                  const next = await reader.read();
                  if (next.done) controller.close();
                  else controller.enqueue(next.value);
                } catch (error) {
                  recorder.add("transport_error", {
                    side,
                    id,
                    error: String(error),
                  });
                  controller.error(error);
                }
              },
              cancel(reason) {
                return reader.cancel(reason);
              },
            })
          : null;
        const responseHeaders = headersFor(response.headers);
        responseHeaders.delete("content-encoding");
        return new Response(observed, {
          status: response.status,
          headers: responseHeaders,
        });
      } catch (e) {
        recorder.add("transport_error", { side, id, error: String(e) });
        return Response.json(
          { error: { message: "E2E transport failed" } },
          { status: 502 },
        );
      }
    },
    websocket: {
      maxPayloadLength: BUDGET,
      open(ws) {
        const remote = ws.data.remote;
        const onOpen = () => {
          ws.data.ready = true;
          recorder.add("upgraded", {
            side: "downstream",
            id: ws.data.id,
            status: 101,
          });
          for (const message of ws.data.queue.splice(0)) remote.send(message);
        };
        remote.addEventListener("open", onOpen);
        remote.addEventListener("message", (ev) => {
          try {
            if (typeof ev.data !== "string")
              throw new Error("binary_websocket_event");
            const event = JSON.parse(ev.data);
            recorder.add("event", {
              side: "downstream",
              id: ws.data.id,
              event,
            });
            if (recorder.exceeded) {
              fail("evidence_limit");
              remote.close();
              ws.close();
              return;
            }
            ws.send(ev.data);
          } catch (e) {
            fail(String(e));
            remote.close();
            ws.close();
          }
        });
        remote.addEventListener("close", (ev) => {
          recorder.add("ws_close", {
            side: "downstream",
            id: ws.data.id,
            code: ev.code,
          });
          sockets.delete(remote);
          ws.close(ev.code === 1006 ? 1011 : ev.code, ev.reason);
        });
        remote.addEventListener("error", () => {
          recorder.add("transport_error", {
            side: "downstream",
            id: ws.data.id,
            error: "websocket_upstream_failed",
          });
          ws.close(1011);
        });
        if (remote.readyState === WebSocket.OPEN) onOpen();
      },
      message(ws, message) {
        try {
          if (typeof message !== "string")
            throw new Error("binary_websocket_event");
          const body = JSON.parse(message);
          recorder.add("request", {
            side: "downstream",
            id: ws.data.id,
            transport: "ws",
            body,
            path: "/v1/responses",
          });
          if (!admit(body, true) || recorder.exceeded) {
            ws.data.remote.close();
            ws.close(1008, "test limit");
            return;
          }
          if (ws.data.ready) ws.data.remote.send(message);
          else {
            if (
              ws.data.queue.reduce((n, s) => n + s.length, 0) + message.length >
              BUDGET
            )
              throw new Error("evidence_limit");
            ws.data.queue.push(message);
          }
        } catch (e) {
          fail(String(e));
          ws.data.remote.close();
          ws.close(1008);
        }
      },
      close(ws) {
        ws.data.remote.close();
        sockets.delete(ws.data.remote);
      },
    },
  });
  return {
    server,
    status: () => ({ fatal, generations, exceeded: recorder.exceeded }),
    stop: () => {
      for (const ws of sockets) ws.close();
      server.stop(true);
    },
  };
}
