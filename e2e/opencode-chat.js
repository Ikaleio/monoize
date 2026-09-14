import { createOpenAICompatible } from "@ai-sdk/openai-compatible";
export function createE2EProvider(options) {
  const sdk = createOpenAICompatible(options);
  return { ...sdk, languageModel: (id) => sdk.chatModel(id) };
}
