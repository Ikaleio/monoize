import {
  ALIAS,
  ENDPOINTS,
  protocol,
  redact,
  type Evidence,
  type Harness,
  type Protocol,
} from "./core";
export interface Execution {
  id?: string;
  input: any;
  output: string;
  ok: boolean;
}
const stringify = (v: any): string =>
  typeof v === "string" ? v : JSON.stringify(v ?? "");
const contentText = (v: any): string =>
  Array.isArray(v)
    ? v.map((x) => x.text ?? "").join("\n")
    : typeof v === "string"
      ? v
      : "";
export function clientEvidence(
  h: Harness,
  events: any[],
): { executions: Execution[]; final: string } {
  const executions: Execution[] = [];
  let final = "";
  const pending = new Map<string, any>();
  for (const ev of events) {
    if (h.startsWith("codex")) {
      if (ev.type === "item.completed" && ev.item?.type === "command_execution")
        executions.push({
          id: ev.item.call_id,
          input: { command: ev.item.command },
          output: ev.item.aggregated_output ?? "",
          ok: ev.item.exit_code === 0,
        });
      if (ev.type === "item.completed" && ev.item?.type === "agent_message")
        final = ev.item.text ?? "";
    } else if (h === "claude-messages") {
      for (const c of ev.message?.content ?? []) {
        if (c.type === "tool_use") pending.set(c.id, c.input);
        if (c.type === "tool_result")
          executions.push({
            id: c.tool_use_id,
            input: pending.get(c.tool_use_id),
            output: contentText(c.content),
            ok: c.is_error !== true,
          });
      }
      if (ev.type === "result" && ev.subtype === "success" && !ev.is_error)
        final = ev.result ?? "";
    } else if (h.startsWith("opencode")) {
      if (ev.type === "tool_use" && ev.part?.state) {
        const p = ev.part;
        executions.push({
          id: p.callID,
          input: p.state.input,
          output: stringify(p.state.output),
          ok: p.state.status === "completed",
        });
      }
      if (ev.type === "text") final += ev.part?.text ?? "";
    } else {
      if (ev.type === "tool_execution_start")
        pending.set(ev.toolCallId, ev.args);
      if (ev.type === "tool_execution_end")
        executions.push({
          id: ev.toolCallId,
          input: pending.get(ev.toolCallId),
          output: contentText(ev.result?.content),
          ok: ev.isError !== true && ev.result?.isError !== true,
        });
      if (
        ev.type === "message_end" &&
        ev.message?.role === "assistant" &&
        ev.message?.stopReason === "stop"
      )
        final = contentText(ev.message.content);
    }
  }
  return { executions, final };
}
interface Call {
  id: string;
  args: any;
}
interface Generation {
  request: Evidence;
  events: Evidence[];
  done: boolean;
  outputId?: string;
  calls: Call[];
}
function parseArgs(v: any) {
  try {
    return typeof v === "string" ? JSON.parse(v) : v;
  } catch {
    return v;
  }
}
function toolResults(body: any, p: Protocol): string[] {
  if (p === "responses")
    return (body?.input ?? [])
      .filter((x: any) =>
        ["function_call_output", "custom_tool_call_output"].includes(x.type),
      )
      .map((x: any) => x.call_id);
  if (p === "chat_completion")
    return (body?.messages ?? [])
      .filter((x: any) => x.role === "tool")
      .map((x: any) => x.tool_call_id);
  return (body?.messages ?? []).flatMap((m: any) =>
    Array.isArray(m.content)
      ? m.content
          .filter((x: any) => x.type === "tool_result")
          .map((x: any) => x.tool_use_id)
      : [],
  );
}
function inspectGeneration(g: Generation, p: Protocol) {
  let incomplete = false;
  const calls = new Map<string, { id: string; args: any }>();
  const chat = new Map<number, { id: string; args: string }>();
  const anthropic = new Map<number, { id: string; args: string; input: any }>();
  for (const record of g.events) {
    const e = record.event;
    if (p === "responses") {
      if (e.type === "response.completed") {
        g.done = true;
        g.outputId = e.response?.id;
      }
      const items =
        e.type === "response.completed"
          ? (e.response?.output ?? [])
          : e.item
            ? [e.item]
            : [];
      for (const i of items)
        if (["function_call", "custom_tool_call"].includes(i.type) && i.call_id)
          calls.set(i.call_id, {
            id: i.call_id,
            args: parseArgs(i.arguments ?? i.input),
          });
    } else if (p === "chat_completion") {
      for (const c of e.choices ?? []) {
        for (const t of c.delta?.tool_calls ?? []) {
          const old = chat.get(t.index) ?? { id: "", args: "" };
          old.id = t.id ?? old.id;
          old.args += t.function?.arguments ?? "";
          chat.set(t.index, old);
        }
        if (
          c.finish_reason &&
          c.finish_reason !== "length" &&
          c.finish_reason !== "content_filter"
        )
          g.done = true;
      }
    } else {
      if (
        e.type === "content_block_start" &&
        e.content_block?.type === "tool_use"
      )
        anthropic.set(e.index, {
          id: e.content_block.id,
          args: "",
          input: e.content_block.input,
        });
      if (
        e.type === "content_block_delta" &&
        e.delta?.type === "input_json_delta"
      ) {
        const c = anthropic.get(e.index);
        if (c) c.args += e.delta.partial_json;
      }
      if (e.type === "message_stop") g.done = true;
      if (
        e.type === "message_delta" &&
        ["max_tokens", "refusal"].includes(e.delta?.stop_reason)
      )
        incomplete = true;
    }
  }
  for (const c of chat.values())
    if (c.id) calls.set(c.id, { id: c.id, args: parseArgs(c.args) });
  for (const c of anthropic.values())
    calls.set(c.id, { id: c.id, args: c.args ? parseArgs(c.args) : c.input });
  g.calls = [...calls.values()];
  if (incomplete) g.done = false;
}
function generations(
  records: Evidence[],
  side: string,
  p: Protocol,
): Generation[] {
  const result: Generation[] = [];
  const current = new Map<string, Generation>();
  for (const r of records) {
    if (r.side !== side) continue;
    if (r.kind === "request" && r.body?.generate === false)
      current.delete(r.id);
    if (r.kind === "request" && r.body?.model && r.body.generate !== false) {
      const g: Generation = { request: r, events: [], done: false, calls: [] };
      result.push(g);
      current.set(r.id, g);
    } else if (r.kind === "event") current.get(r.id)?.events.push(r);
  }
  for (const g of result) inspectGeneration(g, p);
  return result;
}
export function command(v: any): string {
  let value = v?.cmd ?? v?.command ?? "";
  if (Array.isArray(value))
    value =
      value.length >= 3 && /-[a-z]*c$/.test(value[value.length - 2])
        ? value[value.length - 1]
        : value.join(" ");
  if (typeof value !== "string") return "";
  const shell = /^(?:\/[^ ]*\/)?(?:ba|z|da)?sh -[a-z]*c ([\s\S]+)$/.exec(value);
  if (shell) {
    value = shell[1]!;
    if (
      (value.startsWith("'") && value.endsWith("'")) ||
      (value.startsWith('"') && value.endsWith('"'))
    )
      value = value.slice(1, -1);
  }
  return value;
}
function matchesExecution(call: Call, e: Execution) {
  if (e.id === call.id) return true;
  const a = command(call.args),
    b = command(e.input);
  return (
    typeof a === "string" &&
    typeof b === "string" &&
    a.length > 0 &&
    a.trim() === b.trim()
  );
}
export interface Snapshot {
  origin?: string;
  commit?: string;
  document?: string;
  files?: string[];
  error?: string;
}
export function validate(
  h: Harness,
  p: Protocol,
  model: string,
  records: Evidence[],
  exitCode: number | null,
  snapshot: Snapshot,
  repo: string,
  commit: string,
  custom: boolean,
  endpoint?: string,
) {
  const errors: string[] = [];
  const taskErrors: string[] = [];
  const clients = records
    .filter((r) => r.kind === "client")
    .map((r) => r.event);
  const native = clientEvidence(h, clients);
  const down = generations(records, "downstream", protocol(h));
  const up = generations(records, "upstream", p);
  if (!down.length || !up.length) errors.push("missing_generations");
  for (const g of down) {
    if (
      g.request.body.model !== ALIAS ||
      g.request.path !== ENDPOINTS[protocol(h)]
    )
      errors.push("downstream_protocol_or_model");
    if (
      !g.done ||
      (g.request.transport !== "ws" &&
        !records.some(
          (r) =>
            r.kind === "end" &&
            r.side === "downstream" &&
            r.id === g.request.id,
        ))
    )
      errors.push("unfinished_downstream_stream");
  }
  for (const g of up) {
    if (endpoint && g.request.target !== redact(endpoint))
      errors.push("upstream_endpoint");
    if (g.request.body.model !== model || g.request.path !== ENDPOINTS[p])
      errors.push("upstream_protocol_or_model");
    const body = g.request.body;
    if (
      p === "responses"
        ? !Array.isArray(body.input)
        : !Array.isArray(body.messages)
    )
      errors.push("upstream_shape");
    if (
      !g.done ||
      !records.some(
        (r) =>
          r.kind === "end" && r.side === "upstream" && r.id === g.request.id,
      )
    )
      errors.push("unfinished_upstream_stream");
  }
  if (endpoint) {
    const captures = records
      .filter((r) => r.kind === "capture")
      .map((r) => r.capture);
    const expectedDownstream = {
      responses: "responses",
      chat_completion: "chat_completions",
      messages: "anthropic_messages",
    }[protocol(h)];
    for (const g of up) {
      const match = captures.find(
        (c) =>
          c.downstream_protocol === expectedDownstream &&
          typeof c.request_id === "string" &&
          c.attempts?.some(
            (a: any) =>
              a.provider_type === p &&
              a.logical_model === ALIAS &&
              a.upstream_model === model &&
              canonical(a.upstream_request) === canonical(g.request.body),
          ),
      );
      if (!match) errors.push("uncorrelated_monoize_capture");
    }
    for (const response of records.filter(
      (r) =>
        r.kind === "response" &&
        r.side === "downstream" &&
        records.some(
          (q) =>
            q.kind === "request" &&
            q.id === r.id &&
            Object.values(ENDPOINTS).includes(q.path),
        ),
    )) {
      const id = response.headers?.["x-request-id"];
      if (id && !captures.some((c) => c.request_id === id))
        errors.push("missing_request_id_capture");
    }
  }
  if (records.some((r) => r.kind === "capture" && captureTruncated(r.capture)))
    errors.push("capture_truncated");
  if (
    records.some(
      (r) => r.kind === "collection_error" || r.kind === "client_unparsed",
    )
  )
    errors.push("incomplete_evidence");
  if (
    records.some(
      (r) =>
        ["transport_error", "failure"].includes(r.kind) ||
        (r.kind === "http_error" &&
          records.some(
            (q) =>
              q.kind === "request" &&
              q.id === r.id &&
              Object.values(ENDPOINTS).includes(q.path),
          )) ||
        (r.kind === "event" &&
          (r.event?.error ||
            ["error", "response.failed", "response.incomplete"].includes(
              r.event?.type,
            ))),
    )
  )
    errors.push("transport_or_api_error");
  let cycle = false;
  let wsCycle = false;
  for (let i = 0; i < down.length; i++) {
    const g = down[i]!;
    for (const call of g.calls) {
      if (!native.executions.some((e) => e.ok && matchesExecution(call, e)))
        continue;
      for (const next of down.slice(i + 1)) {
        if (
          next.done &&
          toolResults(next.request.body, protocol(h)).includes(call.id)
        ) {
          cycle = true;
          if (
            g.outputId &&
            next.request.body.previous_response_id === g.outputId &&
            next.request.id === g.request.id
          )
            wsCycle = true;
        }
      }
    }
  }
  if (!cycle) {
    errors.push("missing_successful_tool_roundtrip");
    if (native.executions.some((e) => !e.ok)) errors.push("tool_failure");
  }
  let upstreamCycle = false;
  for (let i = 0; i < up.length; i++)
    for (const call of up[i]!.calls)
      if (
        up
          .slice(i + 1)
          .some(
            (g) => g.done && toolResults(g.request.body, p).includes(call.id),
          )
      )
        upstreamCycle = true;
  if (!upstreamCycle) errors.push("missing_upstream_tool_roundtrip");
  if (h === "codex-responses-ws-v2") {
    if (
      !records.some(
        (r) =>
          r.kind === "upgrade" &&
          r.headers?.["openai-beta"]?.includes(
            "responses_websockets=2026-02-06",
          ),
      ) ||
      !records.some((r) => r.kind === "upgraded")
    )
      errors.push("missing_ws_v2_upgrade");
    if (down.some((g) => g.request.transport !== "ws"))
      errors.push("http_fallback");
    if (!wsCycle) errors.push("missing_ws_v2_continuation");
  } else if (down.some((g) => g.request.transport === "ws"))
    errors.push("unexpected_websocket");
  if (exitCode !== 0) taskErrors.push("client_exit");
  if (!native.final.trim()) taskErrors.push("missing_final_response");
  if (!custom) {
    const canonical = (s: string) => s.replace(/\/$/, "").replace(/\.git$/, "");
    if (
      canonical(snapshot.origin ?? "") !== canonical(repo) ||
      snapshot.commit !== commit
    )
      taskErrors.push("repository_identity");
    const clone = native.executions.some(
      (e) =>
        e.ok &&
        /^git\s+clone\s/.test(String(command(e.input)).trim()) &&
        String(command(e.input)).includes(repo),
    );
    if (!clone) taskErrors.push("missing_clone_execution");
    const doc = snapshot.document ?? "";
    if (
      !doc.trim() ||
      !["Entry points", "Modules", "Request flow", "Sources"].every((t) =>
        new RegExp(`^#{1,6} ${t}\\s*$`, "m").test(doc),
      )
    )
      taskErrors.push("architecture_sections");
    const paths = [
      ...new Set(
        [...doc.matchAll(/`([^`\n]+)`/g)]
          .map((m) => m[1]!)
          .filter((s) => snapshot.files?.includes(s)),
      ),
    ];
    if (paths.length < 3) taskErrors.push("source_references");
    const read = paths.filter((file) =>
      native.executions.some((e) => {
        if (!e.ok || !e.output.trim()) return false;
        const p = e.input?.path ?? e.input?.file_path ?? e.input?.filePath;
        if (p === `/work/repo/${file}` || p === file) return true;
        const c = String(command(e.input)).trim();
        return (
          c === `cat -- /work/repo/${file}` ||
          c === `cat -- '/work/repo/${file}'` ||
          c === `cat -- "/work/repo/${file}"`
        );
      }),
    );
    if (read.length < 3) taskErrors.push("source_read_evidence");
    if (snapshot.error) taskErrors.push("snapshot_failed");
  }
  return {
    protocol: {
      passed: errors.length === 0,
      errors: [...new Set(errors)],
      downstreamGenerations: down.length,
      upstreamGenerations: up.length,
    },
    task: {
      passed: taskErrors.length === 0,
      errors: taskErrors,
      repositoryChecks: custom ? "not_applicable" : "performed",
    },
    final: native.final,
  };
}

function captureTruncated(value: any): boolean {
  if (!value || typeof value !== "object") return false;
  if (value.truncated === true) return true;
  return Object.values(value).some(captureTruncated);
}

function canonical(value: any): string {
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  if (value && typeof value === "object")
    return (
      "{" +
      Object.keys(value)
        .sort()
        .map((k) => JSON.stringify(k) + ":" + canonical(value[k]))
        .join(",") +
      "}"
    );
  return JSON.stringify(value);
}
