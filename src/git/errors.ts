/** Native RPC rejections are plain objects, not necessarily Error instances.
 * Only display the public message/code; never stringify opaque response data.
 */
export function gitErrorMessage(
  error: unknown,
  fallback = "The Git operation failed.",
): string {
  if (typeof error === "string") return error.trim() || fallback;
  if (!error || typeof error !== "object") return fallback;
  const message =
    "message" in error && typeof error.message === "string"
      ? error.message.trim()
      : "";
  const code =
    "code" in error &&
    typeof error.code === "string" &&
    /^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)
      ? error.code
      : "";
  if (code === "OUTCOME_UNKNOWN")
    return "The connection was interrupted before the result was confirmed. Check the saved outcome before trying again.";
  if (code === "RECOVERY_REQUIRED")
    return "An interrupted operation needs review before more changes can be made. Check its saved outcome below.";
  if (message) return code ? `${message} (${code})` : message;
  return code ? `${fallback} (${code})` : fallback;
}
