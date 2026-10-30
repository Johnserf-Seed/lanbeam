/** Quick-text size budget (M7.3), mirrored from the backend so the modal can
 *  count and refuse BEFORE a send — the backend's `TextTooLong` is the backstop.
 *  quickText.test.ts reads consts.rs to keep these two numbers in sync. */

/** consts.rs `MAX_TEXT_BYTES`: the cap on the text's UTF-8 bytes (not chars —
 *  a CJK character costs 3). */
export const MAX_TEXT_BYTES = 60 * 1024;

/** consts.rs `MAX_PLAINTEXT`: one Noise message, which the whole `TextSend`
 *  control frame has to fit in. */
const MAX_PLAINTEXT = 65519;

/** Everything in the frame except the escaped text: the kind byte plus
 *  `{"t":"text_send","text":` … `,"to_clipboard":false}` (false, the longer). */
const FRAME_OVERHEAD =
  1 + '{"t":"text_send","text":'.length + ',"to_clipboard":false}'.length;

const enc = new TextEncoder();

/** UTF-8 size of `text`, and whether it passes BOTH backend checks: the raw byte
 *  cap, and the JSON-escaped frame (a `"` or newline costs 2 bytes, a control
 *  char 6 — JSON.stringify escapes exactly as serde_json does). */
export function quickTextSize(text: string): { bytes: number; fits: boolean } {
  const bytes = enc.encode(text).length;
  if (bytes > MAX_TEXT_BYTES) return { bytes, fits: false };
  const frame = FRAME_OVERHEAD + enc.encode(JSON.stringify(text)).length;
  return { bytes, fits: frame <= MAX_PLAINTEXT };
}
