import { readFile, writeFile } from "node:fs/promises";

const root = new URL("./", import.meta.url);
const rows = (await readFile(new URL("observations.jsonl", root), "utf8")).trim().split("\n").map(JSON.parse);
const byId = new Map(rows.map(row => [row.id, row]));
const results = [];
const visit = (value, predicate) => {
  if (predicate(value)) return true;
  return value !== null && typeof value === "object" && Object.values(value).some(child => visit(child, predicate));
};
const has = (value, key, expected) => visit(value, item => item !== null && typeof item === "object" && item[key] === expected);
const check = (id, requirement, predicate, classification = "contract") => {
  const row = byId.get(id);
  if (!row) throw new Error(`Missing fixture: ${id}`);
  results.push({ id, requirement, classification, result: predicate(row) ? "satisfied" : "not_satisfied" });
};

for (const mode of ["nonstream", "stream"]) {
  for (const source of ["chat", "responses"]) {
    check(`${source}-image-data-url-${mode}`, "Gemini encodes image data URLs as inlineData with MIME and raw Base64", row => has(row.encoded.gemini, "mimeType", source === "chat" ? "image/png" : "image/jpeg") && has(row.encoded.gemini, "data", "YXVkaXQgcGF5bG9hZAo=") && !has(row.encoded.gemini, "fileUri", row.canonical.input[0].source.url));
    check(`${source}-pdf-data-url-${mode}`, "PDF MIME and Base64 survive all four target encoders", row => {
      return ["chat", "responses"].every(target => has(row.encoded[target], "file_data", "data:application/pdf;base64,YXVkaXQgcGF5bG9hZAo="))
        && has(row.encoded.messages, "media_type", "application/pdf") && has(row.encoded.messages, "data", "YXVkaXQgcGF5bG9hZAo=")
        && has(row.encoded.gemini, "mimeType", "application/pdf") && has(row.encoded.gemini, "data", "YXVkaXQgcGF5bG9hZAo=");
    }, "positive_control");
    check(`${source}-pdf-raw-base64-${mode}`, "Messages must not emit application/octet-stream as a base64 PDF document source", row => !has(row.encoded.messages, "media_type", "application/octet-stream"));
  }
  check(`messages-content-string-${mode}`, "Valid Messages string custom document survives native decoding", row => row.canonical.input.length > 0 && JSON.stringify(row.canonical).includes("alpha"));
  for (const kind of ["text-source", "content-array"]) {
    for (const target of ["chat", "responses", "gemini"]) {
      check(`messages-${kind}-${mode}`, `${target} preserves available document text or reports an explicit conversion error`, row => JSON.stringify(row.encoded[target]).includes("alpha") || !!row.encoded[target].encode_error, "capability_policy");
    }
  }
  check(`gemini-inline-json-${mode}`, "Messages does not encode application/json as a base64 PDF source", row => !has(row.encoded.messages, "media_type", "application/json"));
  check(`messages-tool-result-media-${mode}`, "Gemini FunctionResponsePart uses only supported inlineData media", row => !visit(row.encoded.gemini, item => !!item?.functionResponse?.parts?.some(part => part.fileData)));
  check(`messages-tool-result-media-${mode}`, "Responses tool result uses input media block shapes", row => has(row.encoded.responses, "type", "input_image") && has(row.encoded.responses, "type", "input_file"), "positive_control");
  for (const [kind, mime] of [["file", "application/pdf"], ["image", "image/jpeg"], ["audio", "audio/wav"]]) {
    check(`urp-url-${kind}-mime-${mode}`, "Gemini preserves typed URL MIME metadata", row => has(row.encoded.gemini, "mimeType", mime), "positive_control");
  }
  check(`gemini-tool-result-file-display-name-${mode}`, "Gemini preserves inline file displayName and matching reference", row => has(row.encoded.gemini, "displayName", "report") && has(row.encoded.gemini, "$ref", "report"), "positive_control");
  check(`responses-file-id-${mode}`, "OpenAI file IDs remain confined to compatible protocol-family outputs", row => has(row.encoded.chat, "file_id", "file_doc") && has(row.encoded.responses, "file_id", "file_doc") && !has(row.encoded.messages, "file_id", "file_doc") && !has(row.encoded.gemini, "file_id", "file_doc"), "scope_control_only");
}
check("chat-generated-audio", "Unknown audio format is not emitted as a literal audio/unknown MIME", row => !has(row.encoded.gemini, "mimeType", "audio/unknown"));
check("gemini-generated-audio", "Gemini PCM MIME parameters survive native response roundtrip", row => has(row.encoded.gemini, "mimeType", "audio/pcm;rate=24000"), "positive_control");

const summary = {
  fixture_rows: rows.length,
  request_scenarios: 40,
  request_stream_flag_variants: 80,
  nonstream_response_scenarios: 2,
  checks: results.length,
  satisfied: results.filter(result => result.result === "satisfied").length,
  not_satisfied: results.filter(result => result.result === "not_satisfied").length,
  limitations: [
    "The stream suffix means request.stream=true; these fixtures do not execute SSE event handling.",
    "Payloads are synthetic bytes; this harness checks structural conversion, not file validity or provider acceptance.",
    "Capability-policy failures record missing adaptation/error behavior, not native support for every exact source shape.",
    "Scope controls check protocol-family provenance only, not provider/account/project ownership."
  ],
  results
};
await writeFile(new URL("check-results.json", root), JSON.stringify(summary, null, 2) + "\n");
console.log(JSON.stringify({ ...summary, results: undefined }, null, 2));
