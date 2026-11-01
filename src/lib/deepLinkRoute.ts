/** Act on a `lanbeam://` link — within the only three things a link is ALLOWED
 *  to do: surface (the backend already did that), PRE-FILL, or NAVIGATE. Never
 *  act. A deep link is untrusted: any web page can ask the OS to open one. See
 *  `deepLink.ts` for the contract; an unknown command is dropped, not guessed. */
import { parseDeepLink } from "./deepLink";
import { useOverlays } from "./store";

export function routeDeepLink(
  raw: string,
  navigate: (path: string) => void,
): void {
  const link = parseDeepLink(raw);
  if (!link) return;
  const ov = useOverlays.getState();
  switch (link.cmd) {
    case "pair":
      // Pre-fills the join view; the user still presses Join and still
      // compares the SAS before anything is trusted.
      ov.setPair(true, { link: link.url });
      break;
    case "text":
      // Pre-fills the body; the user still picks a device and presses send.
      ov.setQt(true, link.text);
      break;
    case "connect":
      // Stages the address in the pairing form's join view. It does NOT dial:
      // the user still chooses Join (with the code) or add-without-pairing.
      ov.setPair(true, { addr: link.addr });
      break;
    case "devices":
      // Devices is the index route — there is no `/devices`.
      navigate("/");
      break;
    case "transfers":
    case "inbox":
    case "settings":
      navigate(`/${link.cmd}`);
      break;
    case "open":
      // The window is already up — that was the whole request.
      break;
  }
}
