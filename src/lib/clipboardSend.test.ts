// Paste-to-send: what a clipboard read turns into, and when the paste chord is
// ours to take. The backend read itself is mocked — what is on the clipboard is
// the input here, not the thing under test.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClipboardPayload, DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useOverlays,
  useRecents,
  useToast,
  useTrust,
  type SendFile,
} from "./store";

vi.mock("../bridge/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../bridge/api")>()),
  readClipboard: vi.fn(),
}));
import { readClipboard } from "../bridge/api";
import {
  installPasteToSend,
  pasteKeys,
  routeClipboard,
  sendFromClipboard,
} from "./clipboardSend";

const overlays0 = { ...useOverlays.getState() };
const data0 = { ...useData.getState() };
const trust0 = { ...useTrust.getState() };
const recents0 = { ...useRecents.getState() };

const mini: DiscoveredDevice = {
  deviceId: "demo-mini",
  name: "Mini",
  address: "192.168.1.31",
  port: 51704,
};
const nano: DiscoveredDevice = { ...mini, deviceId: "demo-nano", name: "Nano" };

const empty: ClipboardPayload = { files: [], image: null, text: null };
const shot = {
  path: "/cache/ab12/clipboard-20261116-143025.png",
  name: "clipboard-20261116-143025.png",
  size: 4096,
  width: 800,
  height: 600,
  thumb: "data:image/png;base64,AAAA",
};
const clip = (over: Partial<ClipboardPayload>): ClipboardPayload => ({
  ...empty,
  ...over,
});

beforeEach(() => {
  vi.mocked(readClipboard).mockReset();
  useOverlays.setState(overlays0, true);
  useData.setState(data0, true);
  useTrust.setState(trust0, true);
  useRecents.setState(recents0, true);
  useToast.getState().hide();
});

describe("what a paste turns into", () => {
  it("a screenshot: the send dialog on the device step, the image selected", () => {
    useData.setState({ devices: [mini, nano] });
    routeClipboard(clip({ image: shot }));

    const send = useOverlays.getState().send;
    expect(send?.step).toBe("device");
    expect(send?.sel).toEqual([shot.path]);
    expect(send?.pool[0]).toMatchObject({
      path: shot.path,
      name: shot.name,
      ext: "png",
      size: 4096,
      thumb: shot.thumb,
      temp: true,
    });
    // Nothing else to say about it.
    expect(useToast.getState().msg).toBeNull();
  });

  it("with one device around there is nothing to choose: straight to confirm", () => {
    useData.setState({ devices: [mini] });
    routeClipboard(clip({ image: shot }));

    const send = useOverlays.getState().send;
    expect(send?.step).toBe("confirm");
    expect(send?.deviceIds).toEqual(["demo-mini"]);
    expect(send?.preset).toBe(true);
  });

  it("…unless that one device is a remembered name under a new key (M19)", () => {
    useData.setState({ devices: [mini] });
    useTrust.setState({
      records: {
        "demo-old": {
          deviceId: "demo-old",
          name: "Mini",
          trusted: true,
          autoAccept: true,
          pairedAt: 1,
          lastSeen: 1,
        },
      },
    } as never);
    routeClipboard(clip({ image: shot }));

    const send = useOverlays.getState().send;
    expect(send?.step).toBe("device");
    expect(send?.deviceIds).toEqual([]);
  });

  it("copied files: the same, by their paths — the icon and name that ride along are ignored", () => {
    useData.setState({ devices: [mini, nano] });
    routeClipboard(
      clip({ files: ["/home/me/a.pdf", "/home/me/Trip"], text: "a.pdf" }),
    );

    const send = useOverlays.getState().send;
    expect(send?.sel).toEqual(["/home/me/a.pdf", "/home/me/Trip"]);
    expect(send?.pool.slice(0, 2).map((f) => f.name)).toEqual([
      "a.pdf",
      "Trip",
    ]);
    expect(useOverlays.getState().qtOpen).toBe(false);
  });

  it("text: the quick-text panel, pre-filled", () => {
    routeClipboard(clip({ text: "https://example.com/x" }));
    expect(useOverlays.getState().qtOpen).toBe(true);
    expect(useOverlays.getState().qtPrefill).toBe("https://example.com/x");
    expect(useOverlays.getState().send).toBeNull();
  });

  it("an image AND text: the image, with the text one click away", () => {
    useData.setState({ devices: [mini, nano] });
    routeClipboard(clip({ image: shot, text: "A1\tB1" }));

    expect(useOverlays.getState().send?.sel).toEqual([shot.path]);
    const toast = useToast.getState();
    expect(toast.msg).toBe(i18n.t("clip.imageTakenToast"));
    expect(toast.action?.label).toBe(i18n.t("clip.sendTextInstead"));

    toast.action?.fn();
    expect(useOverlays.getState().send).toBeNull();
    expect(useOverlays.getState().qtOpen).toBe(true);
    expect(useOverlays.getState().qtPrefill).toBe("A1\tB1");
    expect(useToast.getState().msg).toBeNull();
  });

  it("nothing: says so — unless asked to stay quiet", () => {
    routeClipboard(empty, true);
    expect(useToast.getState().msg).toBeNull();
    routeClipboard(empty);
    expect(useToast.getState().msg).toBe(
      i18n.t("clip.emptyToast", { keys: pasteKeys() }),
    );
    expect(useOverlays.getState().send).toBeNull();
    expect(useOverlays.getState().qtOpen).toBe(false);
  });
});

describe("a paste into a send already being set up", () => {
  const picked: SendFile = {
    path: "/home/me/a.pdf",
    name: "a.pdf",
    ext: "pdf",
  };

  it("adds to the selection and stays on the step it was on", () => {
    useData.setState({ devices: [mini, nano] });
    useOverlays.getState().openSend("demo-nano", [], [picked]);
    routeClipboard(clip({ image: shot }));

    const send = useOverlays.getState().send;
    expect(send?.step).toBe("files");
    expect(send?.deviceIds).toEqual(["demo-nano"]);
    expect(send?.sel).toEqual([picked.path, shot.path]);
    expect(send?.pool.map((f) => f.path)).toEqual([shot.path, picked.path]);
    expect(useToast.getState().msg).toBe(
      i18n.t("clip.addedToast", { count: 1 }),
    );
  });

  it("the same image pasted twice is there once", () => {
    useOverlays.getState().openSend(null, [], [picked]);
    routeClipboard(clip({ image: shot }));
    routeClipboard(clip({ image: shot }));
    const send = useOverlays.getState().send;
    expect(send?.sel).toEqual([picked.path, shot.path]);
    expect(send?.pool).toHaveLength(2);
  });

  it("'send the text instead' takes back only what the paste added", () => {
    useOverlays.getState().openSend(null, [], [picked]);
    routeClipboard(clip({ image: shot, text: "hello" }));
    useToast.getState().action?.fn();

    const send = useOverlays.getState().send;
    expect(send?.sel).toEqual([picked.path]);
    expect(send?.pool.map((f) => f.path)).toEqual([picked.path]);
  });

  it("text is not a file: the selection is left alone and the toast says why", () => {
    useOverlays.getState().openSend(null, [], [picked]);
    routeClipboard(clip({ text: "hello" }));
    expect(useOverlays.getState().qtOpen).toBe(false);
    expect(useOverlays.getState().send?.sel).toEqual([picked.path]);
    expect(useToast.getState().msg).toBe(i18n.t("clip.textNotFileToast"));
  });

  it("once the requests are out, a paste starts a new send instead", () => {
    useData.setState({ devices: [mini, nano] });
    useOverlays.getState().openSend(null, [], [picked]);
    useOverlays.getState().patchSend({ step: "waiting" });
    routeClipboard(clip({ image: shot }));
    const send = useOverlays.getState().send;
    expect(send?.step).toBe("device");
    expect(send?.sel).toEqual([shot.path]);
  });
});

describe("a pasted image is not a recent file", () => {
  it("its cache path will stop existing, so it is not offered again", () => {
    useRecents.getState().add([
      { path: shot.path, name: shot.name, ext: "png", temp: true },
      { path: "/home/me/a.pdf", name: "a.pdf", ext: "pdf" },
    ]);
    expect(useRecents.getState().items.map((f) => f.name)).toEqual(["a.pdf"]);
  });
});

describe("reading the clipboard", () => {
  it("a failed read is reported, not mistaken for an empty clipboard", async () => {
    vi.mocked(readClipboard).mockRejectedValue({ kind: "Io" });
    await sendFromClipboard();
    expect(useToast.getState().msg).toBe(
      i18n.t("clip.readFailedToast", { err: i18n.t("errors.io") }),
    );
  });

  it("a second paste while one is being read is dropped", async () => {
    let release: (v: ClipboardPayload) => void = () => {};
    vi.mocked(readClipboard).mockReturnValue(
      new Promise<ClipboardPayload>((r) => {
        release = r;
      }),
    );
    const first = sendFromClipboard();
    await sendFromClipboard();
    expect(readClipboard).toHaveBeenCalledTimes(1);
    release(clip({ text: "x" }));
    await first;
    expect(useOverlays.getState().qtPrefill).toBe("x");
  });
});

describe("the paste chord", () => {
  let off: () => void;
  beforeEach(() => {
    vi.mocked(readClipboard).mockResolvedValue(empty);
    off = installPasteToSend();
  });
  afterEach(() => off());

  /** Ctrl+V (happy-dom reports a non-Mac platform), aimed at `target`. */
  const chord = (target: EventTarget, init: KeyboardEventInit = {}) => {
    const e = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
      ...init,
    });
    target.dispatchEvent(e);
    return e;
  };

  it("outside a text field it is ours", () => {
    const e = chord(document.body);
    expect(readClipboard).toHaveBeenCalledTimes(1);
    expect(e.defaultPrevented).toBe(true);
  });

  it("inside a text field it is the field's", () => {
    const input = document.body.appendChild(document.createElement("textarea"));
    const e = chord(input);
    expect(readClipboard).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
    input.remove();
  });

  it("other chords on V are left alone", () => {
    chord(document.body, { shiftKey: true });
    chord(document.body, { altKey: true });
    chord(document.body, { ctrlKey: false });
    chord(document.body, { repeat: true });
    expect(readClipboard).not.toHaveBeenCalled();
  });

  it("a dialog asking its own question keeps the keyboard", () => {
    useOverlays.setState({ pairOpen: true });
    chord(document.body);
    useOverlays.setState({ pairOpen: false, qtOpen: true });
    chord(document.body);
    expect(readClipboard).not.toHaveBeenCalled();
  });

  describe("a paste event in a text field", () => {
    const paste = (target: EventTarget, text: string) => {
      const e = new Event("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(e, "clipboardData", {
        value: { getData: () => text },
      });
      target.dispatchEvent(e);
      return e;
    };
    let field: HTMLTextAreaElement;
    beforeEach(() => {
      field = document.body.appendChild(document.createElement("textarea"));
    });
    afterEach(() => field.remove());

    it("with text to paste: the field's, untouched", () => {
      const e = paste(field, "hello");
      expect(readClipboard).not.toHaveBeenCalled();
      expect(e.defaultPrevented).toBe(false);
    });

    it("with no text, into an empty field: send what is there instead", () => {
      const e = paste(field, "");
      expect(readClipboard).toHaveBeenCalledTimes(1);
      expect(e.defaultPrevented).toBe(true);
    });

    it("…but never over a draft already typed", () => {
      field.value = "half a message";
      paste(field, "");
      expect(readClipboard).not.toHaveBeenCalled();
    });
  });
});
