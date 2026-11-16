/** Paste-to-send: Ctrl/⌘+V in the window sends what is on the clipboard.
 *
 *  The case it exists for is the screenshot — Win+Shift+S or ⌃⇧⌘4 leaves an
 *  image on the clipboard and no file anywhere, so there was nothing to drag
 *  and nothing to pick. The backend writes those pixels out as a PNG
 *  (`read_clipboard`); from there a pasted image is a file like any other, and
 *  so the paste lands in the flows that already exist rather than in a third
 *  one:
 *
 *  - copied files, or an image → the send dialog, with them selected (where a
 *    drop onto the window lands too);
 *  - text → the quick-text panel, pre-filled.
 *
 *  Nothing goes out on the paste itself: it stages, and the user still confirms
 *  the device. A clipboard is the one input nobody can see, and a keystroke
 *  that shipped whatever it happened to hold would be a keystroke to fear. */
import i18n from "../i18n";
import * as api from "../bridge/api";
import type { ClipboardPayload } from "../bridge/api";
import { isMac } from "./platform";
import { errText } from "./sendops";
import {
  fpChangedRecordFor,
  sendFileFromPath,
  showToast,
  useData,
  useOverlays,
  useRecents,
  useToast,
  useTrust,
  type SendFile,
} from "./store";

/** The paste chord as this OS spells it, for hints. */
export const pasteKeys = (): string => (isMac() ? "⌘V" : "Ctrl+V");

const keyOf = (f: SendFile) => f.path ?? f.name;

/** Put `files` into the send dialog, selected. Returns how to take them back
 *  out again (the "send the text instead" action). */
function stageFiles(files: SendFile[]): () => void {
  const ov = useOverlays.getState();
  const cur = ov.send;
  // A send being set up: the paste adds to it, like 从电脑选择 does. Once its
  // requests are out ("waiting") it is no longer a selection to add to.
  if (cur && cur.step !== "waiting") {
    const known = new Set(cur.pool.map(keyOf));
    const selected = new Set(cur.sel);
    const fresh = files.filter((f) => !known.has(keyOf(f)));
    const added = files.map(keyOf).filter((k) => !selected.has(k));
    ov.patchSend({
      pool: [...fresh, ...cur.pool],
      sel: [...cur.sel, ...added],
    });
    showToast(i18n.t("clip.addedToast", { count: files.length }));
    return () => {
      const now = useOverlays.getState().send;
      if (!now) return;
      const freshKeys = new Set(fresh.map(keyOf));
      useOverlays.getState().patchSend({
        pool: now.pool.filter((f) => !freshKeys.has(keyOf(f))),
        sel: now.sel.filter((k) => !added.includes(k)),
      });
    };
  }

  // The quick-text panel sits above the send dialog; the paste was not text,
  // so it is not that panel's.
  if (ov.qtOpen) ov.setQt(false);
  const { devices } = useData.getState();
  const recents = useRecents.getState().items;
  // One device around: there is nothing to choose, go straight to confirm.
  // Unless it is a remembered name under a new key — that one has to go
  // through the device step, where it is flagged and cannot be picked (M19).
  const only =
    devices.length === 1 &&
    !fpChangedRecordFor(
      devices[0].deviceId,
      devices,
      useTrust.getState().records,
    )
      ? devices[0]
      : null;
  if (only) {
    ov.openSend(only.deviceId, recents, files);
    ov.patchSend({ step: "confirm" });
  } else {
    ov.openSend(null, recents, files);
    ov.patchSend({ step: "device" });
  }
  return () => useOverlays.getState().closeSend();
}

/** Route what the clipboard held. Exported for tests; callers use
 *  [`sendFromClipboard`]. `quiet` says nothing about an empty clipboard. */
export function routeClipboard(clip: ClipboardPayload, quiet = false): void {
  const ov = useOverlays.getState();
  if (clip.files.length) {
    stageFiles(clip.files.map(sendFileFromPath));
    return;
  }
  const { image, text } = clip;
  if (image) {
    const undo = stageFiles([
      {
        path: image.path,
        name: image.name,
        ext: "png",
        size: image.size,
        thumb: image.thumb,
        temp: true,
      },
    ]);
    // A spreadsheet's cells, a line copied from a document: text AND a picture
    // of itself. Which one was meant is not ours to guess silently — take the
    // picture (what a screenshot is) and offer the text.
    if (text) {
      showToast(
        i18n.t("clip.imageTakenToast"),
        {
          label: i18n.t("clip.sendTextInstead"),
          fn: () => {
            undo();
            useOverlays.getState().setQt(true, text);
            useToast.getState().hide();
          },
        },
        7000,
      );
    }
    return;
  }
  if (text) {
    // A send being set up takes files; text is not something to add to it.
    if (ov.send) {
      showToast(i18n.t("clip.textNotFileToast"));
      return;
    }
    // Already open: its own text box is where text gets pasted.
    if (!ov.qtOpen) ov.setQt(true, text);
    return;
  }
  if (!quiet) showToast(i18n.t("clip.emptyToast", { keys: pasteKeys() }));
}

let reading = false;

/** Read the clipboard and stage what it holds for sending. One read at a time:
 *  a held-down chord must not queue a dozen. */
export async function sendFromClipboard(quiet = false): Promise<void> {
  if (reading) return;
  reading = true;
  try {
    routeClipboard(await api.readClipboard(), quiet);
  } catch (e) {
    showToast(i18n.t("clip.readFailedToast", { err: errText(e) }));
  } finally {
    reading = false;
  }
}

/** The text field a paste is aimed at, if it is one. */
function fieldOf(
  t: EventTarget | null,
): HTMLInputElement | HTMLTextAreaElement | null {
  if (t instanceof HTMLTextAreaElement || t instanceof HTMLInputElement)
    return t;
  return null;
}

const inEditable = (t: EventTarget | null): boolean =>
  !!fieldOf(t) || (t instanceof HTMLElement && t.isContentEditable);

/** A dialog that is asking something of its own is on screen: a paste is not
 *  for the send flow, and a send dialog opening underneath it would be worse
 *  than nothing happening. */
function dialogUp(): boolean {
  const ov = useOverlays.getState();
  return (
    ov.pairOpen ||
    ov.shareOpen ||
    ov.licenseOpen ||
    !!ov.fpAlert ||
    !!ov.conflict
  );
}

/** Listen for the paste chord on `window`. Returns the cleanup.
 *
 *  Two listeners, because "paste" means two things. OUTSIDE a text field
 *  nothing would happen, so the chord is ours — taken on keydown, since a
 *  webview is not obliged to fire `paste` at something that cannot take one.
 *  INSIDE a text field the paste is the field's, and stays so whenever the
 *  clipboard has text to give it; only a paste that would do nothing there (an
 *  image or files, into a field still empty) is treated as "send this". */
export function installPasteToSend(): () => void {
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key.toLowerCase() !== "v" || e.altKey || e.shiftKey || e.repeat)
      return;
    // ⌘V on a Mac; Ctrl+V there is not paste.
    if (!(isMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) return;
    if (inEditable(e.target)) return;
    // The quick-text panel is up but its text box is not focused: text would
    // belong in that box, so leave the chord alone rather than guess.
    if (dialogUp() || useOverlays.getState().qtOpen) return;
    e.preventDefault();
    void sendFromClipboard();
  };
  const onPaste = (e: ClipboardEvent) => {
    const field = fieldOf(e.target);
    if (!field || field.value) return;
    if (e.clipboardData?.getData("text/plain")) return;
    if (dialogUp()) return;
    e.preventDefault();
    // Quiet: pasting nothing into an empty field has always done nothing.
    void sendFromClipboard(true);
  };
  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("paste", onPaste);
  return () => {
    window.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("paste", onPaste);
  };
}
