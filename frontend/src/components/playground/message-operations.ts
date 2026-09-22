import type { FileUIPart, UIMessage } from "ai";

export function latestImageParts(messages: UIMessage[]): FileUIPart[] {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const images = messages[index].parts.filter(
      (part): part is FileUIPart =>
        part.type === "file" &&
        (part.mediaType.startsWith("image/") || part.url.startsWith("data:image/")),
    );
    if (images.length > 0) return images;
  }
  return [];
}

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
