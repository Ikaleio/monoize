import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";
import md5 from "md5";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function getGravatarUrl(email: string | null | undefined, size = 80): string | null {
  if (!email) return null;
  const hash = md5(email.trim().toLowerCase());
  return `https://www.gravatar.com/avatar/${hash}?d=identicon&s=${size}`;
}
