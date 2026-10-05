/** Sample content for the browser demo (`?demo=1` — vite dev, design review,
 *  the README screenshots). It follows the UI language: an English screen full
 *  of Chinese device and file names reads as a bug, not as a demo. Picked at
 *  call time, so it matches whatever language i18n resolved on startup. Never
 *  used inside Tauri. */
import i18n from "../i18n";

export type DemoLang = "en" | "zh";

export type DemoContent = {
  /** this device */
  me: string;
  peers: {
    mini: string;
    phone: string;
    nas: string;
    thinkpad: string;
    ipad: string;
  };
  designZip: string;
  reviewKey: string;
  footageZip: string;
  connectionLost: string;
  textSent: string;
  textReceived: string;
  tripPhotos: string;
  reviewKeyCopy: string;
  homeVideo: string;
  whiteboard: string;
};

const CONTENT: Record<DemoLang, DemoContent> = {
  zh: {
    me: "书房 · MacBook Pro",
    peers: {
      mini: "客厅 · Mac mini",
      phone: "小敏的手机",
      nas: "NAS · Synology",
      thinkpad: "工位 · ThinkPad",
      ipad: "iPad Air",
    },
    designZip: "产品设计稿 v2.zip",
    reviewKey: "季度汇报.key",
    footageZip: "素材包.zip",
    connectionLost: "连接中断",
    textSent: "会议室改到 3 楼 302，记得带 HDMI 线",
    textReceived:
      "https://figma.com/file/lb-review\n评审改到周四 10:30，帮我转给组里",
    tripPhotos: "出差照片 ×24",
    reviewKeyCopy: "季度汇报 (2).key",
    homeVideo: "家庭录像_0705.mov",
    whiteboard: "白板拍照.png",
  },
  en: {
    me: "Study · MacBook Pro",
    peers: {
      mini: "Living room · Mac mini",
      phone: "Mia's iPhone",
      nas: "NAS · Synology",
      thinkpad: "Office · ThinkPad",
      ipad: "iPad Air",
    },
    designZip: "Product design v2.zip",
    reviewKey: "Quarterly review.key",
    footageZip: "Footage.zip",
    connectionLost: "Connection lost",
    textSent:
      "Meeting moved to room 302 on the 3rd floor, bring the HDMI cable",
    textReceived:
      "https://figma.com/file/lb-review\nReview moved to Thursday 10:30, please pass it on",
    tripPhotos: "Trip photos ×24",
    reviewKeyCopy: "Quarterly review (2).key",
    homeVideo: "Family video_0705.mov",
    whiteboard: "Whiteboard.png",
  },
};

export function demoLang(): DemoLang {
  const lng = i18n.resolvedLanguage ?? i18n.language ?? "";
  return lng.startsWith("zh") ? "zh" : "en";
}

export function demoContent(lang: DemoLang = demoLang()): DemoContent {
  return CONTENT[lang];
}
