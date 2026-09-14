import { writeFile } from "node:fs/promises";
const root = new URL("./", import.meta.url);
const cases = [];
const b64 = Buffer.from("audit payload\n").toString("base64");
const data = (mime) => `data:${mime};base64,${b64}`;
const chat = (content, role = "user") => ({ model: "audit", messages: [{ role, content }] });
const responses = (content, role = "user") => ({ model: "audit", input: [{ role, content }] });
const messages = (content, role = "user") => ({ model: "audit", max_tokens: 100, messages: [{ role, content }] });
const gemini = (parts, role = "user") => ({ contents: [{ role, parts }] });
const add = (id, source, request, strip = false) => {
  for (const stream of [false, true]) cases.push({ id: `${id}-${stream ? "stream" : "nonstream"}`, source, request: { ...request, stream }, strip });
};
add("chat-image-data-url", "chat", chat([{ type: "image_url", image_url: { url: data("image/png") } }]));
add("responses-image-data-url", "responses", responses([{ type: "input_image", image_url: data("image/jpeg") }]));
add("chat-image-http", "chat", chat([{ type: "image_url", image_url: { url: "https://example.com/image.png" } }]));
add("messages-image-base64", "messages", messages([{ type: "image", source: { type: "base64", media_type: "image/png", data: b64 } }]));
add("chat-pdf-data-url", "chat", chat([{ type: "file", file: { file_data: data("application/pdf"), filename: "report.pdf" } }]));
add("responses-pdf-data-url", "responses", responses([{ type: "input_file", file_data: data("application/pdf"), filename: "report.pdf" }]));
add("chat-pdf-raw-base64", "chat", chat([{ type: "file", file: { file_data: b64, filename: "report.pdf" } }]));
add("responses-pdf-raw-base64", "responses", responses([{ type: "input_file", file_data: b64, filename: "report.pdf" }]));
add("responses-pdf-url", "responses", responses([{ type: "input_file", file_url: "https://example.com/report.pdf", filename: "report.pdf" }]));
add("messages-pdf-base64", "messages", messages([{ type: "document", source: { type: "base64", media_type: "application/pdf", data: b64 }, title: "Report", context: "Quarterly", citations: { enabled: true } }]));
add("messages-text-source", "messages", messages([{ type: "document", source: { type: "text", media_type: "text/plain", data: "alpha" }, title: "Notes", citations: { enabled: true } }]));
add("messages-content-string", "messages", messages([{ type: "document", source: { type: "content", content: "alpha" }, title: "Notes", citations: { enabled: true } }]));
add("messages-content-array", "messages", messages([{ type: "document", source: { type: "content", content: [{ type: "text", text: "alpha" }] }, title: "Notes", citations: { enabled: true } }]));
for (const mime of ["application/pdf", "text/plain", "application/json", "text/csv", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "audio/wav", "video/mp4"]) {
  const label = mime.split("/")[1].replaceAll(".", "_");
  add(`gemini-inline-${label}`, "gemini", gemini([{ inlineData: { mimeType: mime, data: b64, displayName: "original-file" } }]));
}
for (const [label, mime, uri] of [
  ["pdf-http", "application/pdf", "https://example.com/report.pdf"],
  ["image-http", "image/jpeg", "https://example.com/image.jpg"],
  ["audio-http", "audio/wav", "https://example.com/audio.wav"],
  ["video-http", "video/mp4", "https://example.com/video.mp4"],
  ["pdf-private", "application/pdf", "https://generativelanguage.googleapis.com/v1beta/files/private"],
  ["pdf-gs", "application/pdf", "gs://bucket/report.pdf"],
]) add(`gemini-file-${label}`, "gemini", gemini([{ fileData: { mimeType: mime, fileUri: uri } }]));
add("gemini-video-metadata", "gemini", gemini([{ fileData: { mimeType: "video/mp4", fileUri: "https://example.com/video.mp4" }, videoMetadata: { startOffset: "1s", endOffset: "3s", fps: 1 } }]));
add("responses-image-id", "responses", responses([{ type: "input_image", file_id: "file_image" }]));
add("responses-file-id", "responses", responses([{ type: "input_file", file_id: "file_doc" }]));
add("messages-file-id", "messages", messages([{ type: "document", source: { type: "file", file_id: "file_doc" } }]));
add("messages-image-id", "messages", messages([{ type: "image", source: { type: "file", file_id: "file_image" } }]));
add("urp-url-file-mime", "urp", { model: "audit", input: [{ type: "file", role: "user", source: { type: "url", url: "https://example.com/report.pdf" }, metadata: { media_type: "application/pdf" } }] });
add("urp-url-image-mime", "urp", { model: "audit", input: [{ type: "image", role: "user", source: { type: "url", url: "https://example.com/image.jpg" }, metadata: { media_type: "image/jpeg" } }] });
add("urp-url-audio-mime", "urp", { model: "audit", input: [{ type: "audio", role: "user", source: { type: "url", url: "https://example.com/audio.wav" }, metadata: { media_type: "audio/wav" } }] });
const resultContent = [{ type: "text", text: "look" }, { type: "image", source: { type: "url", url: "https://example.com/image.png" } }, { type: "document", source: { type: "base64", media_type: "application/pdf", data: b64 } }];
add("messages-tool-result-media", "messages", { model: "audit", max_tokens: 100, messages: [{ role: "assistant", content: [{ type: "tool_use", id: "c", name: "inspect", input: {} }] }, { role: "user", content: [{ type: "tool_result", tool_use_id: "c", content: resultContent }] }] });
add("gemini-tool-result-display-name", "gemini", gemini([{ functionResponse: { id: "c", name: "inspect", response: { output: { $ref: "screenshot" } }, parts: [{ inlineData: { mimeType: "image/png", data: b64, displayName: "screenshot" } }] } }]));
add("gemini-tool-result-file-display-name", "gemini", gemini([{ functionResponse: { id: "c", name: "inspect", response: { output: { $ref: "report" } }, parts: [{ inlineData: { mimeType: "application/pdf", data: b64, displayName: "report" } }] } }]));
add("responses-tool-result-file", "responses", { model: "audit", input: [{ type: "function_call", call_id: "c", name: "inspect", arguments: "{}" }, { type: "function_call_output", call_id: "c", output: [{ type: "input_file", file_data: data("application/pdf"), filename: "report.pdf" }] }] });
add("gemini-model-image", "gemini", gemini([{ inlineData: { mimeType: "image/png", data: b64 } }], "model"));
add("gemini-model-file", "gemini", gemini([{ inlineData: { mimeType: "application/pdf", data: b64 } }], "model"));
for (const [id, source, response] of [
  ["gemini-generated-audio", "gemini", { candidates: [{ content: { role: "model", parts: [{ inlineData: { mimeType: "audio/pcm;rate=24000", data: b64 } }] }, finishReason: "STOP" }] }],
  ["chat-generated-audio", "chat", { id: "r", model: "audio", choices: [{ index: 0, message: { role: "assistant", content: null, audio: { id: "a", data: b64, transcript: "words", expires_at: 1000 } }, finish_reason: "stop" }] }],
]) cases.push({ id, source, mode: "response", response });
await writeFile(new URL("fixtures.jsonl", root), cases.map(value => JSON.stringify(value)).join("\n") + "\n");
console.log(`Wrote ${cases.length} cases. Payload bytes are synthetic; probes validate transport structure, not media decoding.`);
