import type { FileUIPart, UIMessage } from "ai";

export function filePartsForEditedUserMessage(
  messages: UIMessage[],
  messageId: string,
): FileUIPart[] {
  const message = messages.find(
    (candidate) => candidate.id === messageId && candidate.role === "user",
  );
  if (!message) return [];
  return message.parts
    .filter((part): part is FileUIPart => part.type === "file")
    .map((part) => ({ ...part }));
}
