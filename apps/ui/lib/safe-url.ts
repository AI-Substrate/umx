/** An http(s) URL as a string, or null. Nothing else ever becomes a link (security note B5). */
export function safeHttpUrl(value: string): string | null {
  if (!/^https?:\/\//i.test(value.trim())) return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}
